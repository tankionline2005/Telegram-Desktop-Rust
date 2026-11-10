//! `tdr` — console client for exercising the protocol stack.
//!
//! The session lives in `data/session.bin` (git-ignored). Environment:
//! `TDR_KEEP_TEMP_KEYS=0` forgets temporary (PFS) keys on exit, as tdesktop does;
//! `TDR_LOG=info` logs.

use std::io::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use qrcode::QrCode;
use qrcode::render::unicode;
use tdr_api::{Client, Config, Error, SessionData};
use tdr_tl::{enums, functions, types};

const USAGE: &str = "\
usage:
  tdr ping            connect to Telegram with fresh keys and time each step
  tdr login           log in by scanning a QR code (Settings → Devices → Link Desktop Device)
  tdr login phone     log in with a phone number and a code
  tdr me              show the logged-in account (with connection timings)
  tdr dialogs [N]     list the first N chats (default 20)
  tdr avatar          download your profile photo to data/avatar.jpg
  tdr logout          log out and delete the local session

environment:
  TDR_KEEP_TEMP_KEYS=0   forget temporary (PFS) keys on exit, as tdesktop does (slower start)
  TDR_LOG=info           show connection logs";

fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data")
}

fn session_path() -> PathBuf {
    data_dir().join("session.bin")
}

fn config() -> Result<Config> {
    let (Some(id), Some(hash)) = (option_env!("TDR_API_ID"), option_env!("TDR_API_HASH")) else {
        bail!(
            "API credentials are missing: copy .env.example to .env, fill in api_id/api_hash \
             from https://my.telegram.org and rebuild"
        );
    };
    let mut c = Config::new(id.parse().context("TDR_API_ID must be a number")?, hash);
    if std::env::var("TDR_KEEP_TEMP_KEYS").is_ok_and(|v| v == "0") {
        c.persist_temp_keys = false;
    }
    Ok(c)
}

fn load_session() -> Result<Option<SessionData>> {
    match std::fs::read(session_path()) {
        Ok(d) => Ok(Some(SessionData::from_bytes(&d)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn save_session(client: &Client) -> Result<()> {
    std::fs::create_dir_all(data_dir())?;
    std::fs::write(session_path(), client.session_bytes())?;
    Ok(())
}

/// Client for a logged-in account.
fn logged_in() -> Result<Client> {
    let session = load_session()?.context("not logged in; run `tdr login` first")?;
    if !session.is_authorized() {
        bail!("the saved session is not logged in; run `tdr login`");
    }
    Ok(Client::new(config()?, session))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("TDR_LOG")
                .unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["ping"] => ping().await,
        ["login"] | ["login", "qr"] => login(LoginMethod::Qr).await,
        ["login", "phone"] => login(LoginMethod::Phone).await,
        ["me"] => me().await,
        ["dialogs"] => dialogs(20).await,
        ["dialogs", n] => dialogs(n.parse().context("N must be a number")?).await,
        ["avatar"] => avatar().await,
        ["logout"] => logout().await,
        ["selftest", "reconnect"] => selftest_reconnect().await,
        ["selftest", "unknown-key"] => selftest_unknown_key().await,
        ["selftest", "dc", n] => selftest_dc(n.parse().context("DC must be a number")?).await,
        _ => {
            println!("{USAGE}");
            Ok(())
        }
    }
}

fn ms(d: Duration) -> String {
    format!("{:.1} ms", d.as_secs_f64() * 1000.0)
}

async fn ping() -> Result<()> {
    let client = Client::new(config()?, SessionData::new(2));
    let t = Instant::now();
    let enums::NearestDc::NearestDc(dc) = client.invoke(&functions::help::GetNearestDc {}).await?;
    println!(
        "Cold start to first answer (permanent + temporary key, bind, initConnection): {}",
        ms(t.elapsed())
    );
    println!("  this DC {}, nearest DC {}", dc.this_dc, dc.nearest_dc);

    let t = Instant::now();
    let enums::Config::Config(cfg) = client.invoke(&functions::help::GetConfig {}).await?;
    println!(
        "help.getConfig: {} ({} DC options)",
        ms(t.elapsed()),
        cfg.dc_options.len()
    );

    let t = Instant::now();
    let n = 20;
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..n {
        let c = client.clone();
        tasks.spawn(async move { c.invoke(&functions::help::GetNearestDc {}).await });
    }
    while let Some(r) = tasks.join_next().await {
        r??;
    }
    println!(
        "{n} parallel requests (batched into containers): {}",
        ms(t.elapsed())
    );
    Ok(())
}

fn base64url(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..=chunk.len() {
            out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

fn print_qr(token: &[u8]) -> Result<()> {
    let url = format!("tg://login?token={}", base64url(token));
    let code = QrCode::new(url.as_bytes())?;
    let image = code
        .render::<unicode::Dense1x2>()
        .dark_color(unicode::Dense1x2::Light)
        .light_color(unicode::Dense1x2::Dark)
        .build();
    // Clear the screen so a refreshed code replaces the old one.
    print!("\x1b[2J\x1b[H");
    println!("{image}");
    println!("Scan with Telegram on your phone: Settings → Devices → Link Desktop Device.");
    println!("Unofficial Telegram Desktop Rust uses the Telegram API.");
    Ok(())
}

fn prompt(label: &str) -> Result<String> {
    print!("{label}");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

enum LoginMethod {
    Qr,
    Phone,
}

fn is_rpc(e: &Error, name: &str) -> bool {
    matches!(e, Error::Rpc(r) if r.is(name))
}

async fn login(method: LoginMethod) -> Result<()> {
    if load_session()?.is_some_and(|s| s.is_authorized()) {
        bail!("already logged in; run `tdr logout` first");
    }
    let client = Client::new(config()?, SessionData::new(2));
    let authorization = match method {
        LoginMethod::Qr => login_qr(&client).await?,
        LoginMethod::Phone => login_phone(&client).await?,
    };
    let enums::auth::Authorization::Authorization(a) = authorization else {
        bail!("this phone number has no Telegram account; sign up in an official app first");
    };
    client.mark_authorized();
    save_session(&client)?;
    println!(
        "\nLogged in as {} (DC {}).",
        user_name(&a.user),
        client.home_dc()
    );
    Ok(())
}

async fn login_qr(client: &Client) -> Result<enums::auth::Authorization> {
    let cfg = client.config().clone();
    let mut shown_token = Vec::new();
    loop {
        let request = functions::auth::ExportLoginToken {
            api_id: cfg.api_id,
            api_hash: cfg.api_hash.clone(),
            except_ids: vec![],
        };
        let token = match client.invoke(&request).await {
            Ok(t) => t,
            Err(e) if is_rpc(&e, "SESSION_PASSWORD_NEEDED") => return check_password(client).await,
            Err(e) => return Err(e.into()),
        };
        match token {
            enums::auth::LoginToken::LoginToken(t) => {
                if t.token[..] != shown_token[..] {
                    print_qr(&t.token)?;
                    shown_token = t.token.to_vec();
                }
                // `updateLoginToken` (or any update) wakes us up; otherwise re-check soon.
                client.next_event(Duration::from_secs(5)).await;
            }
            enums::auth::LoginToken::MigrateTo(m) => {
                client.set_home_dc(m.dc_id);
                match client
                    .invoke(&functions::auth::ImportLoginToken { token: m.token })
                    .await
                {
                    Ok(enums::auth::LoginToken::Success(s)) => return Ok(s.authorization),
                    Ok(_) => bail!("unexpected answer to auth.importLoginToken"),
                    Err(e) if is_rpc(&e, "SESSION_PASSWORD_NEEDED") => {
                        return check_password(client).await;
                    }
                    Err(e) => return Err(e.into()),
                }
            }
            enums::auth::LoginToken::Success(s) => return Ok(s.authorization),
        }
    }
}

async fn login_phone(client: &Client) -> Result<enums::auth::Authorization> {
    let cfg = client.config().clone();
    let phone = prompt("Phone number (international format, e.g. +7…): ")?;
    let sent = client
        .invoke(&functions::auth::SendCode {
            phone_number: phone.clone(),
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
    let (hash, kind) = match sent {
        enums::auth::SentCode::SentCode(s) => (s.phone_code_hash, s.r#type),
        enums::auth::SentCode::Success(s) => return Ok(s.authorization),
        enums::auth::SentCode::PaymentRequired(_) => {
            bail!("Telegram requires a payment to send the code to this number");
        }
    };
    let where_ = match kind {
        enums::auth::SentCodeType::App(_) => "in Telegram on your other device",
        enums::auth::SentCodeType::Sms(_)
        | enums::auth::SentCodeType::SmsWord(_)
        | enums::auth::SentCodeType::SmsPhrase(_) => "by SMS",
        enums::auth::SentCodeType::Call(_) => "by a phone call",
        enums::auth::SentCodeType::EmailCode(_) => "by email",
        _ => "",
    };
    loop {
        let code = prompt(&format!("Code (sent {where_}): "))?;
        let result = client
            .invoke(&functions::auth::SignIn {
                phone_number: phone.clone(),
                phone_code_hash: hash.clone(),
                phone_code: Some(code),
                email_verification: None,
            })
            .await;
        match result {
            Ok(a) => return Ok(a),
            Err(e) if is_rpc(&e, "SESSION_PASSWORD_NEEDED") => return check_password(client).await,
            Err(e) if is_rpc(&e, "PHONE_CODE_INVALID") => println!("Wrong code, try again."),
            Err(e) => return Err(e.into()),
        }
    }
}

async fn check_password(client: &Client) -> Result<enums::auth::Authorization> {
    let enums::account::Password::Password(pw) =
        client.invoke(&functions::account::GetPassword {}).await?;
    let algo = match pw.current_algo {
        Some(enums::PasswordKdfAlgo::SHA256SHA256PBKDF2HMACSHA512iter100000SHA256ModPow(a)) => a,
        _ => bail!("unsupported password algorithm; update the client"),
    };
    let (Some(srp_b), Some(srp_id)) = (pw.srp_b, pw.srp_id) else {
        bail!("server did not send SRP parameters");
    };
    let hint = pw.hint.map(|h| format!(" (hint: {h})")).unwrap_or_default();
    loop {
        let password = rpassword::prompt_password(format!("Cloud password{hint}: "))?;
        let params = tdr_crypto::srp::SrpParams {
            salt1: &algo.salt1,
            salt2: &algo.salt2,
            g: algo.g as u32,
            p: &algo.p,
            srp_b: &srp_b,
        };
        // PBKDF2 with 100k iterations: keep it off the async runtime threads.
        let (s1, s2, pw) = (
            algo.salt1.clone(),
            algo.salt2.clone(),
            password.into_bytes(),
        );
        let hash =
            tokio::task::spawn_blocking(move || tdr_crypto::srp::password_hash(&pw, &s1, &s2))
                .await?;
        let answer = tdr_crypto::srp::compute_with_hash(&params, &hash)
            .context("bad SRP parameters from server")?;
        let check = functions::auth::CheckPassword {
            password: types::InputCheckPasswordSRP {
                srp_id,
                a: answer.a.into(),
                m1: answer.m1.into(),
            }
            .into(),
        };
        match client.invoke(&check).await {
            Ok(auth) => return Ok(auth),
            Err(e) if is_rpc(&e, "PASSWORD_HASH_INVALID") => println!("Wrong password, try again."),
            Err(e) => return Err(e.into()),
        }
    }
}

fn user_name(user: &enums::User) -> String {
    match user {
        enums::User::User(u) => {
            let mut name = u.first_name.clone().unwrap_or_default();
            if let Some(last) = &u.last_name {
                name.push(' ');
                name.push_str(last);
            }
            if let Some(username) = &u.username {
                name.push_str(&format!(" (@{username})"));
            }
            name
        }
        enums::User::Empty(u) => format!("deleted user {}", u.id),
    }
}

async fn get_me(client: &Client) -> Result<enums::User> {
    let mut users = client
        .invoke(&functions::users::GetUsers {
            id: vec![enums::InputUser::InputUserSelf],
        })
        .await?;
    users.pop().context("server returned no user")
}

async fn me() -> Result<()> {
    let client = logged_in()?;
    let had_temp_key = client
        .session()
        .dcs
        .get(&client.home_dc())
        .is_some_and(|d| d.temp.is_some());
    let t = Instant::now();
    let user = get_me(&client).await?;
    let elapsed = t.elapsed();
    save_session(&client)?;
    println!("{}", user_name(&user));
    eprintln!(
        "\nstart → answer: {} (DC {}, {})",
        ms(elapsed),
        client.home_dc(),
        if had_temp_key {
            "saved temporary key"
        } else {
            "new temporary key"
        }
    );
    Ok(())
}

async fn dialogs(limit: i32) -> Result<()> {
    let client = logged_in()?;
    let t = Instant::now();
    let result = client
        .invoke(&functions::messages::GetDialogs {
            exclude_pinned: false,
            folder_id: None,
            offset_date: 0,
            offset_id: 0,
            offset_peer: enums::InputPeer::Empty,
            limit,
            hash: 0,
        })
        .await?;
    let elapsed = t.elapsed();
    save_session(&client)?;
    let (dialogs, chats, users) = match result {
        enums::messages::Dialogs::Dialogs(d) => (d.dialogs, d.chats, d.users),
        enums::messages::Dialogs::Slice(d) => (d.dialogs, d.chats, d.users),
        enums::messages::Dialogs::NotModified(_) => (vec![], vec![], vec![]),
    };
    let user_title = |id: i64| {
        users.iter().find_map(|u| match u {
            enums::User::User(x) if x.id == id => Some(user_name(u)),
            _ => None,
        })
    };
    let chat_title = |id: i64| {
        chats.iter().find_map(|c| match c {
            enums::Chat::Chat(x) if x.id == id => Some(x.title.clone()),
            enums::Chat::Channel(x) if x.id == id => Some(x.title.clone()),
            enums::Chat::Forbidden(x) if x.id == id => Some(x.title.clone()),
            enums::Chat::ChannelForbidden(x) if x.id == id => Some(x.title.clone()),
            _ => None,
        })
    };
    for d in &dialogs {
        let enums::Dialog::Dialog(d) = d else {
            continue;
        };
        let (kind, title) = match &d.peer {
            enums::Peer::User(p) => ("user", user_title(p.user_id)),
            enums::Peer::Chat(p) => ("group", chat_title(p.chat_id)),
            enums::Peer::Channel(p) => {
                let megagroup = chats.iter().any(
                    |c| matches!(c, enums::Chat::Channel(x) if x.id == p.channel_id && x.megagroup),
                );
                (
                    if megagroup { "group" } else { "channel" },
                    chat_title(p.channel_id),
                )
            }
        };
        let unread = if d.unread_count > 0 {
            format!("  [{}]", d.unread_count)
        } else {
            String::new()
        };
        println!(
            "{:<8} {}{unread}",
            kind,
            title.unwrap_or_else(|| "?".into())
        );
    }
    eprintln!("\n{} chats in {}", dialogs.len(), ms(elapsed));
    Ok(())
}

async fn avatar() -> Result<()> {
    let client = logged_in()?;
    let t = Instant::now();
    let user = get_me(&client).await?;
    let enums::User::User(u) = &user else {
        bail!("no user")
    };
    let Some(enums::UserProfilePhoto::UserProfilePhoto(photo)) = &u.photo else {
        println!("You have no profile photo.");
        return Ok(());
    };
    let home = client.home_dc();
    println!("Profile photo is on DC {} (home DC {home}).", photo.dc_id);

    let location: enums::InputFileLocation = types::InputPeerPhotoFileLocation {
        big: true,
        peer: enums::InputPeer::InputPeerSelf,
        photo_id: photo.photo_id,
    }
    .into();
    // upload.getFile: the limit must divide 1 MiB and offsets must be multiples of it.
    const PART: i32 = 512 * 1024;
    let mut data = Vec::new();
    let t_file = Instant::now();
    let mut first_part = None;
    loop {
        let request = functions::upload::GetFile {
            precise: false,
            cdn_supported: false,
            location: location.clone(),
            offset: data.len() as i64,
            limit: PART,
        };
        let part = match client.invoke_in(photo.dc_id, &request).await? {
            enums::upload::File::File(f) => f.bytes,
            enums::upload::File::CdnRedirect(_) => bail!("CDN downloads are not supported yet"),
        };
        first_part.get_or_insert(t_file.elapsed());
        let done = part.len() < PART as usize;
        data.extend_from_slice(&part);
        if done {
            break;
        }
    }
    let path = data_dir().join("avatar.jpg");
    std::fs::write(&path, &data)?;
    save_session(&client)?;
    println!("Saved {} KB to {}", data.len() / 1024, path.display());
    eprintln!(
        "\nfirst part from DC {} (incl. connecting there): {}\nwhole file: {}; total with getUsers: {}",
        photo.dc_id,
        ms(first_part.unwrap_or_default()),
        ms(t_file.elapsed()),
        ms(t.elapsed())
    );
    Ok(())
}

/// Breaks the connection while requests are in flight and checks they still complete.
async fn selftest_reconnect() -> Result<()> {
    let client = logged_in()?;
    get_me(&client).await?;
    let home = client.home_dc();
    for round in 1..=3 {
        let t = Instant::now();
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..10 {
            let c = client.clone();
            tasks.spawn(async move { c.invoke(&functions::help::GetNearestDc {}).await });
        }
        client.simulate_network_failure(home).await?;
        let mut ok = 0;
        while let Some(r) = tasks.join_next().await {
            r??;
            ok += 1;
        }
        let reconnected = loop {
            match client.next_event(Duration::from_secs(5)).await {
                Some(tdr_mtproto::Event::Reconnected) => break true,
                Some(_) => continue,
                None => break false,
            }
        };
        println!(
            "round {round}: socket dropped mid-flight, {ok}/10 requests answered in {}, reconnect event: {reconnected}",
            ms(t.elapsed())
        );
    }
    save_session(&client)?;
    Ok(())
}

/// Uses the account on another DC: cold (keys + authorization import), warm, and with the
/// connection prepared in the background while the home DC is busy.
///
/// The probe asks DC `dc` for the profile photo. If the photo lives elsewhere the server
/// answers `FILE_MIGRATE_X`, which still proves the request was authorized there.
async fn selftest_dc(dc: i32) -> Result<()> {
    async fn probe(client: &Client, dc: i32, location: &enums::InputFileLocation) -> Result<()> {
        let request = functions::upload::GetFile {
            precise: false,
            cdn_supported: false,
            location: location.clone(),
            offset: 0,
            limit: 4096,
        };
        match client.invoke_in(dc, &request).await {
            Ok(_) => Ok(()),
            Err(Error::Rpc(e)) if e.is("FILE_MIGRATE") => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    let client = logged_in()?;
    let user = get_me(&client).await?;
    let enums::User::User(u) = &user else {
        bail!("no user")
    };
    let Some(enums::UserProfilePhoto::UserProfilePhoto(photo)) = &u.photo else {
        bail!("this test needs a profile photo");
    };
    let location: enums::InputFileLocation = types::InputPeerPhotoFileLocation {
        big: false,
        peer: enums::InputPeer::InputPeerSelf,
        photo_id: photo.photo_id,
    }
    .into();
    let known = client.session().dcs.get(&dc).is_some_and(|k| k.authorized);

    let t = Instant::now();
    probe(&client, dc, &location).await?;
    println!(
        "DC {dc}, first request ({}): {}",
        if known {
            "authorization already imported"
        } else {
            "new keys + authorization import"
        },
        ms(t.elapsed())
    );
    let t = Instant::now();
    probe(&client, dc, &location).await?;
    println!("DC {dc}, second request: {}", ms(t.elapsed()));
    save_session(&client)?;

    // Fresh starts where the app knows it will need DC `dc` (e.g. media stored there).
    let client = logged_in()?;
    let started = Instant::now();
    client.prepare_dc(dc);
    get_me(&client).await?;
    let t = Instant::now();
    probe(&client, dc, &location).await?;
    println!(
        "fresh start, DC {dc} prepared in background: request there {}, start → answer {}",
        ms(t.elapsed()),
        ms(started.elapsed())
    );
    save_session(&client)?;
    let client = logged_in()?;
    let started = Instant::now();
    get_me(&client).await?;
    let t = Instant::now();
    probe(&client, dc, &location).await?;
    println!(
        "fresh start, DC {dc} not prepared:         request there {}, start → answer {}",
        ms(t.elapsed()),
        ms(started.elapsed())
    );
    save_session(&client)?;
    Ok(())
}

/// Replaces the saved temporary key with one the server has never seen (as if it expired
/// or was destroyed) and checks that the client recovers by creating a new one.
async fn selftest_unknown_key() -> Result<()> {
    let mut session = load_session()?.context("not logged in")?;
    let home = session.home_dc;
    let keys = session
        .dcs
        .get_mut(&home)
        .context("no keys for the home DC")?;
    keys.temp = Some(tdr_api::TempKey {
        key: tdr_crypto::AuthKey::from_bytes(tdr_crypto::random_bytes()),
        expires_at: i32::MAX,
        salt: 0,
    });
    let client = Client::new(config()?, session);
    let t = Instant::now();
    let user = get_me(&client).await?;
    println!(
        "server rejected the bogus key, client created a new one: answered in {} ({})",
        ms(t.elapsed()),
        if matches!(user, enums::User::User(_)) {
            "ok"
        } else {
            "unexpected"
        }
    );
    save_session(&client)?;
    Ok(())
}

async fn logout() -> Result<()> {
    let client = logged_in()?;
    match client.invoke(&functions::auth::LogOut {}).await {
        Ok(_) => {}
        Err(Error::Rpc(e)) if e.code == 401 => {}
        Err(e) => return Err(e.into()),
    }
    match std::fs::remove_file(session_path()) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
        _ => {}
    }
    println!("Logged out.");
    Ok(())
}
