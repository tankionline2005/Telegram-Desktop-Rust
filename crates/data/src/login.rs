//! Logging in (tdesktop's `Intro`): by QR code, or by phone number with a code and
//! the cloud password. [`run`] drives one login with commands from the window and
//! reports what to show.

use std::time::Duration;

use tdr_api::{Client, Error};
use tdr_tl::{enums, functions, types};
use tokio::sync::mpsc;

/// What the window asks for.
#[derive(Debug, Clone)]
pub enum LoginCommand {
    /// Show QR codes (the first step, and "Log in using QR code").
    Qr,
    /// Stop the QR codes (the phone step is shown).
    StopQr,
    /// Send the code to this number (digits with a leading "+").
    Phone(String),
    Code(String),
    Password(String),
    /// "Send code via SMS" (`auth.resendCode`).
    ResendCode,
}

/// A country of the phone step (`help.countriesList`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Country {
    pub iso2: String,
    pub name: String,
    /// Calling code without "+", and the number patterns ("XXX XXX XXXX").
    pub code: String,
    pub patterns: Vec<String>,
}

/// What the window shows.
#[derive(Debug, Clone)]
pub enum LoginEvent {
    /// The countries and the one the server thinks we are in.
    Countries { list: Vec<Country>, nearest: String },
    /// A new QR code: `tg://login?token=…`.
    QrCode(String),
    /// The code was sent: where, and how many digits it has.
    CodeSent {
        phone: String,
        via_app: bool,
        length: usize,
        /// "Send code via SMS" is possible.
        can_resend: bool,
    },
    /// The account has a cloud password.
    PasswordNeeded { hint: String },
    /// Something to show under the step (tdesktop's texts).
    Error(String),
    /// Logged in: the session is saved.
    Done,
}

/// tdesktop's error texts (`lng_bad_phone`, `lng_bad_code`, ...).
fn error_text(e: &Error) -> String {
    let rpc = |name: &str| matches!(e, Error::Rpc(r) if r.is(name));
    if rpc("PHONE_NUMBER_INVALID") {
        "Invalid phone number. Please try again.".into()
    } else if rpc("PHONE_NUMBER_BANNED") {
        "This phone number is banned.".into()
    } else if rpc("PHONE_CODE_INVALID") || rpc("PHONE_CODE_EMPTY") {
        "You have entered an invalid code.".into()
    } else if rpc("PHONE_CODE_EXPIRED") {
        "The code has expired. Please try again.".into()
    } else if rpc("PASSWORD_HASH_INVALID") {
        "You have entered a wrong password.".into()
    } else if rpc("FLOOD_WAIT") || matches!(e, Error::Rpc(r) if r.code == 420) {
        "Too many tries. Please try again later.".into()
    } else {
        format!("{e}")
    }
}

fn code_length(kind: &enums::auth::SentCodeType) -> usize {
    use enums::auth::SentCodeType as T;
    let length = match kind {
        T::App(t) => t.length,
        T::Sms(t) => t.length,
        T::Call(t) => t.length,
        T::MissedCall(t) => t.length,
        T::EmailCode(t) => t.length,
        T::FragmentSms(t) => t.length,
        T::FirebaseSms(t) => t.length,
        _ => 5,
    };
    length.clamp(1, 32) as usize
}

struct Phone {
    number: String,
    hash: String,
}

/// Drives one login until it succeeds (the session is written to `session_path`) or
/// the window goes away.
pub async fn run(
    client: Client,
    session_path: std::path::PathBuf,
    mut commands: mpsc::UnboundedReceiver<LoginCommand>,
    events: mpsc::UnboundedSender<LoginEvent>,
) {
    let send = |e: LoginEvent| {
        let _ = events.send(e);
    };
    // The phone step's countries (tdesktop has them built in; the server has the
    // same list).
    {
        let client = client.clone();
        let events = events.clone();
        tokio::spawn(async move {
            let nearest = match client.invoke(&functions::help::GetNearestDc {}).await {
                Ok(enums::NearestDc::NearestDc(n)) => n.country,
                Err(_) => String::new(),
            };
            let list = match client
                .invoke(&functions::help::GetCountriesList {
                    lang_code: "en".into(),
                    hash: 0,
                })
                .await
            {
                Ok(enums::help::CountriesList::CountriesList(l)) => l
                    .countries
                    .iter()
                    .flat_map(|c| {
                        let enums::help::Country::Country(c) = c;
                        let name = c.name.clone().unwrap_or_else(|| c.default_name.clone());
                        c.country_codes
                            .iter()
                            .map(|code| {
                                let enums::help::CountryCode::CountryCode(code) = code;
                                Country {
                                    iso2: c.iso2.clone(),
                                    name: name.clone(),
                                    code: code.country_code.clone(),
                                    patterns: code.patterns.clone().unwrap_or_default(),
                                }
                            })
                            .collect::<Vec<_>>()
                    })
                    .collect(),
                _ => Vec::new(),
            };
            let _ = events.send(LoginEvent::Countries { list, nearest });
        });
    }

    let mut qr = true;
    let mut shown_token: Vec<u8> = Vec::new();
    let mut phone: Option<Phone> = None;
    let mut password: Option<types::account::Password> = None;
    loop {
        // QR codes are asked for while that step is shown, again when one expires or
        // an update (`updateLoginToken`) says it was scanned.
        let mut qr_wait = Duration::from_secs(3600);
        if qr {
            match export_token(&client).await {
                Ok(Exported::Token { token, expires_in }) => {
                    if token != shown_token {
                        send(LoginEvent::QrCode(format!(
                            "tg://login?token={}",
                            base64url(&token)
                        )));
                        shown_token = token;
                    }
                    qr_wait = Duration::from_secs(expires_in.clamp(1, 30));
                }
                Ok(Exported::Done) => {
                    finish(&client, &session_path, &events);
                    return;
                }
                Ok(Exported::Password) => match get_password(&client).await {
                    Ok(p) => {
                        qr = false;
                        send(LoginEvent::PasswordNeeded {
                            hint: p.hint.clone().unwrap_or_default(),
                        });
                        password = Some(p);
                    }
                    Err(e) => send(LoginEvent::Error(error_text(&e))),
                },
                Err(e) => {
                    send(LoginEvent::Error(error_text(&e)));
                    qr_wait = Duration::from_secs(5);
                }
            }
        }
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { return };
                match command {
                    LoginCommand::Qr => {
                        qr = true;
                        shown_token.clear();
                    }
                    LoginCommand::StopQr => qr = false,
                    LoginCommand::Phone(number) => {
                        qr = false;
                        match send_code(&client, &number).await {
                            Ok(Sent::Code { hash, via_app, length, can_resend }) => {
                                send(LoginEvent::CodeSent {
                                    phone: number.clone(),
                                    via_app,
                                    length,
                                    can_resend,
                                });
                                phone = Some(Phone { number, hash });
                            }
                            Ok(Sent::Done) => {
                                finish(&client, &session_path, &events);
                                return;
                            }
                            Err(e) => send(LoginEvent::Error(error_text(&e))),
                        }
                    }
                    LoginCommand::ResendCode => {
                        if let Some(p) = &phone {
                            let resent = client
                                .invoke(&functions::auth::ResendCode {
                                    phone_number: p.number.clone(),
                                    phone_code_hash: p.hash.clone(),
                                    reason: None,
                                })
                                .await;
                            match resent {
                                Ok(enums::auth::SentCode::SentCode(s)) => {
                                    send(LoginEvent::CodeSent {
                                        phone: p.number.clone(),
                                        via_app: matches!(s.r#type, enums::auth::SentCodeType::App(_)),
                                        length: code_length(&s.r#type),
                                        can_resend: s.next_type.is_some(),
                                    });
                                    phone = Some(Phone { number: p.number.clone(), hash: s.phone_code_hash });
                                }
                                Ok(_) => {}
                                Err(e) => send(LoginEvent::Error(error_text(&e))),
                            }
                        }
                    }
                    LoginCommand::Code(code) => {
                        let Some(p) = &phone else { continue };
                        let result = client
                            .invoke(&functions::auth::SignIn {
                                phone_number: p.number.clone(),
                                phone_code_hash: p.hash.clone(),
                                phone_code: Some(code),
                                email_verification: None,
                            })
                            .await;
                        match result {
                            Ok(enums::auth::Authorization::Authorization(_)) => {
                                finish(&client, &session_path, &events);
                                return;
                            }
                            Ok(enums::auth::Authorization::SignUpRequired(_)) => {
                                send(LoginEvent::Error(
                                    "This number has no Telegram account yet: sign up in an \
                                     official app first."
                                        .into(),
                                ));
                            }
                            Err(e) if matches!(&e, Error::Rpc(r) if r.is("SESSION_PASSWORD_NEEDED")) => {
                                match get_password(&client).await {
                                    Ok(p) => {
                                        send(LoginEvent::PasswordNeeded {
                                            hint: p.hint.clone().unwrap_or_default(),
                                        });
                                        password = Some(p);
                                    }
                                    Err(e) => send(LoginEvent::Error(error_text(&e))),
                                }
                            }
                            Err(e) => send(LoginEvent::Error(error_text(&e))),
                        }
                    }
                    LoginCommand::Password(text) => {
                        let Some(p) = &password else { continue };
                        match check_password(&client, p, text).await {
                            Ok(()) => {
                                finish(&client, &session_path, &events);
                                return;
                            }
                            Err(e) => {
                                send(LoginEvent::Error(error_text(&e)));
                                // A new SRP round needs fresh parameters.
                                if let Ok(fresh) = get_password(&client).await {
                                    password = Some(fresh);
                                }
                            }
                        }
                    }
                }
            }
            _ = client.next_event(qr_wait), if qr => {}
        }
    }
}

fn finish(client: &Client, path: &std::path::Path, events: &mpsc::UnboundedSender<LoginEvent>) {
    client.mark_authorized();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match client.save_session(path) {
        Ok(()) => {
            let _ = events.send(LoginEvent::Done);
        }
        Err(e) => {
            let _ = events.send(LoginEvent::Error(format!(
                "Could not save the session: {e}"
            )));
        }
    }
}

enum Exported {
    Token { token: Vec<u8>, expires_in: u64 },
    Done,
    Password,
}

fn is_password_needed(e: &Error) -> bool {
    matches!(e, Error::Rpc(r) if r.is("SESSION_PASSWORD_NEEDED"))
}

/// `auth.exportLoginToken`, following a migration to another DC.
async fn export_token(client: &Client) -> Result<Exported, Error> {
    let cfg = client.config().clone();
    let answer = client
        .invoke(&functions::auth::ExportLoginToken {
            api_id: cfg.api_id,
            api_hash: cfg.api_hash.clone(),
            except_ids: vec![],
        })
        .await;
    let token = match answer {
        Ok(t) => t,
        Err(e) if is_password_needed(&e) => return Ok(Exported::Password),
        Err(e) => return Err(e),
    };
    match token {
        enums::auth::LoginToken::LoginToken(t) => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as i64);
            let left = (i64::from(t.expires) - (now + i64::from(client_offset(client)))).max(1);
            Ok(Exported::Token {
                token: t.token.to_vec(),
                expires_in: left as u64,
            })
        }
        enums::auth::LoginToken::MigrateTo(m) => {
            client.set_home_dc(m.dc_id);
            match client
                .invoke(&functions::auth::ImportLoginToken { token: m.token })
                .await
            {
                Ok(enums::auth::LoginToken::Success(_)) => Ok(Exported::Done),
                Ok(_) => Err(Error::Security(
                    "unexpected answer to auth.importLoginToken",
                )),
                Err(e) if is_password_needed(&e) => Ok(Exported::Password),
                Err(e) => Err(e),
            }
        }
        enums::auth::LoginToken::Success(_) => Ok(Exported::Done),
    }
}

fn client_offset(client: &Client) -> i32 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64) as i32;
    client.server_time() - now
}

enum Sent {
    Code {
        hash: String,
        via_app: bool,
        length: usize,
        can_resend: bool,
    },
    Done,
}

async fn send_code(client: &Client, number: &str) -> Result<Sent, Error> {
    let cfg = client.config().clone();
    let sent = client
        .invoke(&functions::auth::SendCode {
            phone_number: number.to_string(),
            api_id: cfg.api_id,
            api_hash: cfg.api_hash.clone(),
            settings: types::CodeSettings {
                allow_flashcall: false,
                current_number: false,
                allow_app_hash: false,
                allow_missed_call: false,
                allow_firebase: false,
                unknown_number: false,
                logout_tokens: None,
                token: None,
                app_sandbox: None,
            }
            .into(),
        })
        .await?;
    match sent {
        enums::auth::SentCode::SentCode(s) => Ok(Sent::Code {
            via_app: matches!(s.r#type, enums::auth::SentCodeType::App(_)),
            length: code_length(&s.r#type),
            can_resend: s.next_type.is_some(),
            hash: s.phone_code_hash,
        }),
        enums::auth::SentCode::Success(_) => Ok(Sent::Done),
        enums::auth::SentCode::PaymentRequired(_) => Err(Error::Security(
            "Telegram requires a payment to send the code to this number",
        )),
    }
}

async fn get_password(client: &Client) -> Result<types::account::Password, Error> {
    let enums::account::Password::Password(p) =
        client.invoke(&functions::account::GetPassword {}).await?;
    Ok(*p)
}

/// SRP check of the cloud password (`auth.checkPassword`).
async fn check_password(
    client: &Client,
    pw: &types::account::Password,
    password: String,
) -> Result<(), Error> {
    let Some(enums::PasswordKdfAlgo::SHA256SHA256PBKDF2HMACSHA512iter100000SHA256ModPow(algo)) =
        pw.current_algo.clone()
    else {
        return Err(Error::Security("unsupported password algorithm"));
    };
    let (Some(srp_b), Some(srp_id)) = (pw.srp_b.clone(), pw.srp_id) else {
        return Err(Error::Security("the server sent no SRP parameters"));
    };
    let (s1, s2, bytes) = (
        algo.salt1.clone(),
        algo.salt2.clone(),
        password.into_bytes(),
    );
    // PBKDF2 with 100 000 rounds: off the async threads.
    let hash =
        tokio::task::spawn_blocking(move || tdr_crypto::srp::password_hash(&bytes, &s1, &s2))
            .await
            .map_err(|_| Error::Disconnected)?;
    let params = tdr_crypto::srp::SrpParams {
        salt1: &algo.salt1,
        salt2: &algo.salt2,
        g: algo.g as u32,
        p: &algo.p,
        srp_b: &srp_b,
    };
    let answer = tdr_crypto::srp::compute_with_hash(&params, &hash)
        .ok_or(Error::Security("bad SRP parameters from the server"))?;
    client
        .invoke(&functions::auth::CheckPassword {
            password: types::InputCheckPasswordSRP {
                srp_id,
                a: answer.a.into(),
                m1: answer.m1.into(),
            }
            .into(),
        })
        .await?;
    Ok(())
}

/// Base64url without padding (the QR login link).
fn base64url(data: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(data.len() * 4 / 3 + 3);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let chars = chunk.len() + 1;
        for i in 0..chars {
            out.push(ABC[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

/// The phone formatted by the country's pattern (`Ui::FormatPhone`): "+7 900 123 45 67".
pub fn format_phone(digits: &str, countries: &[Country]) -> String {
    let digits: String = digits.chars().filter(char::is_ascii_digit).collect();
    // The longest calling code that starts the number.
    let Some(country) = countries
        .iter()
        .filter(|c| !c.code.is_empty() && digits.starts_with(&c.code))
        .max_by_key(|c| c.code.len())
    else {
        return format!("+{digits}");
    };
    let rest = &digits[country.code.len()..];
    let mut out = format!("+{}", country.code);
    let pattern = country
        .patterns
        .iter()
        .find(|p| p.chars().filter(|c| *c != ' ').count() == rest.len())
        .or_else(|| country.patterns.first());
    match pattern {
        Some(pattern) => {
            let mut chars = rest.chars();
            out.push(' ');
            for p in pattern.chars() {
                if p == ' ' {
                    out.push(' ');
                } else if let Some(c) = chars.next() {
                    out.push(c);
                } else {
                    break;
                }
            }
            out.extend(chars);
        }
        None if !rest.is_empty() => {
            out.push(' ');
            out.push_str(rest);
        }
        None => {}
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64url_without_padding() {
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"fo"), "Zm8");
        assert_eq!(base64url(b"foo"), "Zm9v");
        assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
    }

    #[test]
    fn phones_like_tdesktop() {
        let countries = vec![
            Country {
                iso2: "RU".into(),
                name: "Russian Federation".into(),
                code: "7".into(),
                patterns: vec!["XXX XXX XX XX".into()],
            },
            Country {
                iso2: "US".into(),
                name: "USA".into(),
                code: "1".into(),
                patterns: vec!["XXX XXX XXXX".into()],
            },
        ];
        assert_eq!(format_phone("79001234567", &countries), "+7 900 123 45 67");
        assert_eq!(format_phone("+1 555 0100", &countries), "+1 555 010 0");
        assert_eq!(format_phone("999", &countries), "+999");
    }
}
