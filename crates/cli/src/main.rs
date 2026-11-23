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
use tdr_data::tdr_storage::{
    DialogView, FileCache, FileCacheConfig, MessageRow, PeerKey, PeerKind, Storage,
};
use tdr_data::{Applied, Sync};
use tdr_tl::{enums, functions, types};

const USAGE: &str = "\
usage:
  tdr ping            connect to Telegram with fresh keys and time each step
  tdr login           log in by scanning a QR code (Settings → Devices → Link Desktop Device)
  tdr login phone     log in with a phone number and a code
  tdr me              show the logged-in account (with connection timings)
  tdr dialogs [N]     the first N chats: instantly from the cache, then synced (default 20)
  tdr history I [N]   the last N messages (default 30) of chat number I from `tdr dialogs`
  tdr files [clear]   size of the file cache, or clear it
  tdr watch [SECS]    print new messages as they arrive (Ctrl+C to stop)
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
    client.save_session(&session_path())?;
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
        ["history", n] => history(n.parse().context("chat number")?, 30).await,
        ["history", n, count] => {
            history(
                n.parse().context("chat number")?,
                count.parse().context("count")?,
            )
            .await
        }
        ["files"] => files_info(),
        ["files", "clear"] => files_clear(),
        ["watch"] => watch(None).await,
        ["watch", secs] => watch(Some(secs.parse().context("seconds must be a number")?)).await,
        ["avatar"] => avatar().await,
        ["probe", "dialogs", needle] => probe_dialogs(needle).await,
        ["probe", "saved", offset] => probe_saved(offset.parse().context("message id")?).await,
        ["probe", "peer-dialog", id] => probe_peer_dialog(id.parse().context("user id")?).await,
        ["probe", "keep", id] => probe_keep(id.parse().context("user id")?, true).await,
        ["probe", "relist", id] => probe_keep(id.parse().context("user id")?, false).await,
        ["probe", "entities"] => probe_entities(),
        ["probe", "bank-card", number] => probe_bank_card(number).await,
        ["probe", "folders"] => probe_folders().await,
        ["probe", "emoji"] => probe_emoji().await,
        ["probe", "emoji-speed"] => probe_emoji_speed().await,
        ["probe", "saved-emoji", dir] => probe_saved_emoji(dir).await,
        ["probe", "login"] => probe_login().await,
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
    match load_session() {
        Ok(Some(s)) if s.is_authorized() => bail!("already logged in; run `tdr logout` first"),
        Ok(_) => {}
        // A damaged session (e.g. cut short by a crash) is as good as none: log in anew.
        Err(e) => eprintln!("The saved session is damaged ({e}); logging in again."),
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

fn cache_path() -> PathBuf {
    data_dir().join("cache.sqlite")
}

fn open_cache() -> Result<Storage> {
    std::fs::create_dir_all(data_dir())?;
    Ok(Storage::open(cache_path())?)
}

fn truncate(s: &str, max: usize) -> String {
    let line = s.lines().next().unwrap_or("");
    if line.chars().count() > max {
        format!("{}…", line.chars().take(max).collect::<String>())
    } else {
        line.to_string()
    }
}

fn print_dialogs(list: &[DialogView]) {
    for d in list {
        let kind = match (
            d.dialog.peer.kind,
            d.peer.as_ref().is_some_and(|p| p.is_megagroup),
        ) {
            (PeerKind::User, _) => "user",
            (PeerKind::Chat, _) | (PeerKind::Channel, true) => "group",
            (PeerKind::Channel, false) => "channel",
        };
        let name = d.peer.as_ref().map_or("?", |p| p.name.as_str());
        let unread = if d.dialog.unread_count > 0 {
            format!(" [{}]", d.dialog.unread_count)
        } else {
            String::new()
        };
        let pin = if d.dialog.pinned { "📌 " } else { "" };
        let last = d
            .top
            .as_ref()
            .map(|m| truncate(&m.text, 40))
            .unwrap_or_default();
        println!("{kind:<8} {pin}{}{unread}  — {last}", truncate(name, 40));
    }
}

fn describe(applied: &Applied) -> String {
    format!(
        "{} new, {} edited, {} deleted{}",
        applied.new_messages.len(),
        applied.edited,
        applied.deleted,
        if applied.reloaded {
            ", chat list (re)loaded"
        } else {
            ""
        }
    )
}

/// Pages through the server's main chat list like `Sync` does and reports the
/// paging (no names, no session or cache writes): where a chat whose user name
/// contains `needle` comes, and where the paging stops.
async fn probe_dialogs(needle: &str) -> Result<()> {
    let client = logged_in()?;
    let needle = needle.to_lowercase();
    let (mut offset_date, mut offset_id, mut offset_peer) = (0, 0, enums::InputPeer::Empty);
    let mut total = 0;
    for page in 0..50 {
        let r = client
            .invoke(&functions::messages::GetDialogs {
                exclude_pinned: false,
                folder_id: None,
                offset_date,
                offset_id,
                offset_peer: offset_peer.clone(),
                limit: 100,
                hash: 0,
            })
            .await?;
        let (dialogs, messages, users, chats, slice, count) = match r {
            enums::messages::Dialogs::Dialogs(d) => {
                (d.dialogs, d.messages, d.users, d.chats, false, 0)
            }
            enums::messages::Dialogs::Slice(d) => {
                (d.dialogs, d.messages, d.users, d.chats, true, d.count)
            }
            enums::messages::Dialogs::NotModified(_) => {
                println!("page {page}: not modified");
                break;
            }
        };
        let user = |id: i64| {
            users.iter().find_map(|u| match u {
                enums::User::User(u) if u.id == id => Some(u),
                _ => None,
            })
        };
        let mut last = None;
        let mut kinds = [0; 3];
        for (i, d) in dialogs.iter().enumerate() {
            let d = match d {
                enums::Dialog::Dialog(d) => d,
                enums::Dialog::Folder(_) => {
                    kinds[1] += 1;
                    continue;
                }
                enums::Dialog::Community(_) => {
                    kinds[2] += 1;
                    continue;
                }
            };
            kinds[0] += 1;
            let date = messages.iter().find_map(|m| {
                let (id, date, peer) = match m {
                    enums::Message::Message(m) => (m.id, m.date, &m.peer_id),
                    enums::Message::Service(m) => (m.id, m.date, &m.peer_id),
                    enums::Message::Empty(_) => return None,
                };
                (id == d.top_message && peer == &d.peer).then_some(date)
            });
            if let enums::Peer::User(p) = &d.peer
                && let Some(u) = user(p.user_id)
            {
                let name = format!(
                    "{} {}",
                    u.first_name.clone().unwrap_or_default(),
                    u.last_name.clone().unwrap_or_default()
                )
                .to_lowercase();
                if name.contains(&needle) {
                    println!(
                        "  match: page {page}, index {i}, date {date:?}, pinned {}",
                        d.pinned
                    );
                }
            }
            last = Some((d.peer.clone(), d.top_message, date));
        }
        total += kinds[0];
        let Some((peer, top, date)) = last else {
            println!("page {page}: no chats");
            break;
        };
        let input = match &peer {
            enums::Peer::User(p) => user(p.user_id).and_then(|u| u.access_hash).map(|h| {
                enums::InputPeer::User(types::InputPeerUser {
                    user_id: p.user_id,
                    access_hash: h,
                })
            }),
            enums::Peer::Chat(p) => Some(enums::InputPeer::Chat(types::InputPeerChat {
                chat_id: p.chat_id,
            })),
            enums::Peer::Channel(p) => chats.iter().find_map(|c| match c {
                enums::Chat::Channel(c) if c.id == p.channel_id => c.access_hash.map(|h| {
                    enums::InputPeer::Channel(types::InputPeerChannel {
                        channel_id: p.channel_id,
                        access_hash: h,
                    })
                }),
                _ => None,
            }),
        };
        println!(
            "page {page}: dialogs {} folders {} communities {} slice {slice} count {count}, last date {date:?}, last has input {}",
            kinds[0],
            kinds[1],
            kinds[2],
            input.is_some()
        );
        let (Some(input), Some(date)) = (input, date) else {
            break;
        };
        if dialogs.len() < 100 || !slice {
            break;
        }
        (offset_date, offset_id, offset_peer) = (date, top, input);
    }
    println!("total chats {total}");
    Ok(())
}

/// What `messages.getHistory` of Saved Messages answers below `offset` (no texts,
/// no writes): the answer type, its count and the ids.
async fn probe_saved(offset: i32) -> Result<()> {
    let client = logged_in()?;
    for (offset_id, add_offset, limit) in [
        (offset, 0, 100),
        (offset + 1, 0, 5),
        (offset - 1, 0, 20),
        (offset - 100, 0, 20),
        (10000, 0, 20),
        (1000, 0, 20),
        (offset, -10, 20),
    ] {
        let r = client
            .invoke(&functions::messages::GetHistory {
                peer: enums::InputPeer::InputPeerSelf,
                offset_id,
                offset_date: 0,
                add_offset,
                limit,
                max_id: 0,
                min_id: 0,
                hash: 0,
            })
            .await?;
        let (kind, messages, count) = match r {
            enums::messages::Messages::Messages(m) => ("messages", m.messages, None),
            enums::messages::Messages::Slice(m) => ("slice", m.messages, Some(m.count)),
            enums::messages::Messages::ChannelMessages(m) => ("channel", m.messages, Some(m.count)),
            enums::messages::Messages::NotModified(_) => ("not modified", vec![], None),
        };
        let ids: Vec<(i32, &str)> = messages
            .iter()
            .map(|m| match m {
                enums::Message::Message(m) => (m.id, "msg"),
                enums::Message::Service(m) => (m.id, "service"),
                enums::Message::Empty(m) => (m.id, "empty"),
            })
            .collect();
        println!(
            "offset {offset_id} limit {limit}: {kind}, count {count:?}, {} messages, first {:?} last {:?}",
            ids.len(),
            ids.first(),
            ids.last()
        );
    }
    Ok(())
}

/// `payments.getBankCardData` for a card number (use a public test number): the
/// answer or the error.
async fn probe_bank_card(number: &str) -> Result<()> {
    let client = logged_in()?;
    for n in [number.to_string(), number.replace(' ', "")] {
        let r = client
            .invoke(&functions::payments::GetBankCardData { number: n.clone() })
            .await;
        match r {
            Ok(enums::payments::BankCardData::BankCardData(d)) => {
                println!("{n:?}: title {:?}, {} links", d.title, d.open_urls.len())
            }
            Err(e) => println!("{n:?}: error {e}"),
        }
    }
    Ok(())
}

/// A login as the window starts it (`tdr_data::login::run`): the kinds of the first
/// events only (no tokens, no numbers). The session goes to a temporary file.
async fn probe_login() -> Result<()> {
    use tdr_data::login::{LoginEvent, run};
    let client = Client::new(config()?, SessionData::new(2));
    let path = std::env::temp_dir().join("tdr-probe-login.bin");
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let (ev_tx, mut ev_rx) = tokio::sync::mpsc::unbounded_channel();
    let task = tokio::spawn(run(client, path.clone(), true, cmd_rx, ev_tx));
    let started = Instant::now();
    let mut seen = 0;
    while seen < 2 {
        match tokio::time::timeout(Duration::from_secs(20), ev_rx.recv()).await {
            Ok(Some(LoginEvent::QrCode(url))) => {
                println!(
                    "QR code after {}: {} chars, tg://login link: {}",
                    ms(started.elapsed()),
                    url.len(),
                    url.starts_with("tg://login?token=")
                );
                seen += 1;
            }
            Ok(Some(LoginEvent::Countries { list, nearest })) => {
                println!(
                    "countries after {}: {} codes, nearest known: {}",
                    ms(started.elapsed()),
                    list.len(),
                    !nearest.is_empty()
                );
                seen += 1;
            }
            Ok(Some(LoginEvent::Error(e))) => println!("error: {e}"),
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => break,
        }
    }
    drop(cmd_tx);
    task.abort();
    let _ = std::fs::remove_file(path);
    Ok(())
}

/// The account's folders: per folder its icon, rule sizes and how many cached chats
/// it holds (no titles, no chat names).
/// The custom emoji pipeline on real data: installed emoji sets, the first set's
/// documents, a few files downloaded and decoded. Prints counts, formats and
/// timings only.
async fn probe_emoji() -> Result<()> {
    use tdr_data::emoji_sets::{self, EmojiFormat, SetRef};
    let client = logged_in()?;
    let sets = emoji_sets::installed_emoji_sets(&client).await?;
    println!("installed emoji sets: {}", sets.len());
    let featured = emoji_sets::featured_emoji_sets(&client).await?;
    println!("featured emoji sets: {}", featured.len());
    let mut by_docs = std::collections::BTreeMap::<usize, usize>::new();
    for f in &featured {
        *by_docs.entry(f.documents.len()).or_default() += 1;
    }
    println!("featured sets by documents given: {by_docs:?}");
    println!(
        "featured already installed: {}",
        featured
            .iter()
            .filter(|f| sets.iter().any(|s| s.id == f.info.id))
            .count()
    );
    let Some(first) = sets.first().or(featured.first().map(|f| &f.info)) else {
        return Ok(());
    };
    let raw = emoji_sets::sticker_set(&client, &SetRef::Id(first.id, first.access_hash)).await?;
    let full = emoji_sets::full_set(&raw).ok_or_else(|| anyhow::anyhow!("no set"))?;
    println!(
        "first set: {} documents, {} packs",
        full.documents.len(),
        full.packs.len()
    );
    let mut docs: Vec<_> = Vec::new();
    for set in sets.iter().chain(featured.iter().map(|f| &f.info).take(30)) {
        let raw = emoji_sets::sticker_set(&client, &SetRef::Id(set.id, set.access_hash)).await?;
        if let Some(full) = emoji_sets::full_set(&raw) {
            let all: Vec<_> = full
                .documents
                .iter()
                .filter_map(emoji_sets::emoji_document)
                .collect();
            docs.extend(all.iter().take(1).cloned());
            docs.extend(
                all.iter()
                    .filter(|d| d.format == EmojiFormat::Webm)
                    .take(1)
                    .cloned(),
            );
        }
    }
    let mut seen = std::collections::HashSet::new();
    for doc in docs
        .iter()
        .filter(|d| seen.insert(format!("{:?}", d.format)))
        .chain(docs.iter().take(2))
    {
        let t = std::time::Instant::now();
        let bytes = emoji_sets::download_document(&client, doc).await?;
        let downloaded = t.elapsed();
        let t = std::time::Instant::now();
        let frames = match doc.format {
            EmojiFormat::Tgs => tdr_lottie::Animation::from_tgs(&bytes)
                .map(|a| a.frame_count())
                .map_err(|e| e.to_string()),
            EmojiFormat::Webm => tdr_webm::decode(&bytes, 200)
                .map(|d| {
                    d.frames.len() * 1000 + tdr_webm::demux(&bytes).map_or(0, |v| v.blocks.len())
                })
                .map_err(|e| e.to_string()),
            EmojiFormat::Webp => Ok(1),
        };
        println!(
            "{:?} {} bytes in {downloaded:?}, text_color {}, decoded {frames:?} in {:?}",
            doc.format,
            bytes.len(),
            doc.text_color,
            t.elapsed()
        );
    }
    let ids: Vec<i64> = docs.iter().take(3).map(|d| d.id).collect();
    let again = emoji_sets::custom_emoji_documents(&client, &ids).await?;
    println!("documents by id: {} of {}", again.len(), ids.len());
    Ok(())
}

/// Times the stages of custom emoji loading on the first installed sets (counts
/// and timings only): documents, downloads at a few concurrencies, decoding and
/// rendering every frame at the panel's size, and whether the frames cover the
/// animation's whole length.
async fn probe_emoji_speed() -> Result<()> {
    use std::time::{Duration, Instant};
    use tdr_data::emoji_sets::{self, EmojiFormat, SetRef};
    let client = logged_in()?;
    let sets = emoji_sets::installed_emoji_sets(&client).await?;
    let mut docs = Vec::new();
    let webm_only = std::env::var("PROBE_WEBM").is_ok();
    let featured = emoji_sets::featured_emoji_sets(&client).await?;
    let all_sets: Vec<_> = sets
        .iter()
        .cloned()
        .chain(featured.iter().map(|f| f.info.clone()))
        .collect();
    for set in all_sets.iter().take(if webm_only { 40 } else { 3 }) {
        let t = Instant::now();
        let raw = emoji_sets::sticker_set(&client, &SetRef::Id(set.id, set.access_hash)).await?;
        if let Some(full) = emoji_sets::full_set(&raw) {
            println!(
                "set: {} documents in {:?}",
                full.documents.len(),
                t.elapsed()
            );
            let list = full.documents.iter().filter_map(emoji_sets::emoji_document);
            if webm_only {
                docs.extend(list.filter(|d| d.format == EmojiFormat::Webm).take(2));
            } else {
                docs.extend(list.take(24));
            }
        }
        if webm_only && docs.len() >= 24 {
            break;
        }
    }
    let ids: Vec<i64> = docs.iter().map(|d| d.id).collect();
    let t = Instant::now();
    let again = emoji_sets::custom_emoji_documents(&client, &ids).await?;
    println!(
        "getCustomEmojiDocuments: {} in {:?}",
        again.len(),
        t.elapsed()
    );
    let dcs: std::collections::BTreeSet<i32> = docs.iter().map(|d| d.dc_id).collect();
    println!("document dcs: {dcs:?}");
    let mut files: Vec<Vec<u8>> = vec![Vec::new(); docs.len()];
    for concurrency in [1usize, 4, 16] {
        let chunk: Vec<usize> = (0..docs.len()).collect();
        let t = Instant::now();
        let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(concurrency));
        let mut tasks = Vec::new();
        for &k in chunk.iter().take(16) {
            let client = client.clone();
            let doc = docs[k].clone();
            let sem = sem.clone();
            tasks.push(tokio::spawn(async move {
                let _p = sem.acquire().await;
                let t = Instant::now();
                let r = emoji_sets::download_document(&client, &doc).await;
                (k, r, t.elapsed())
            }));
        }
        let mut lat: Vec<Duration> = Vec::new();
        for task in tasks {
            let (k, r, d) = task.await?;
            lat.push(d);
            if let Ok(bytes) = r {
                files[k] = bytes;
            }
        }
        lat.sort();
        println!(
            "download 16 at concurrency {concurrency}: total {:?}, latency min {:?} median {:?} max {:?}",
            t.elapsed(),
            lat[0],
            lat[lat.len() / 2],
            lat[lat.len() - 1]
        );
    }
    for (k, doc) in docs.iter().enumerate().skip(16) {
        files[k] = emoji_sets::download_document(&client, doc)
            .await
            .unwrap_or_default();
    }
    let side = 34u32;
    for (doc, bytes) in docs.iter().zip(&files) {
        if bytes.is_empty() {
            continue;
        }
        let t = Instant::now();
        match doc.format {
            EmojiFormat::Tgs => {
                let Ok(anim) = tdr_lottie::Animation::from_tgs(bytes) else {
                    println!("tgs: bad");
                    continue;
                };
                let parsed = t.elapsed();
                let mut pixmap = tdr_lottie::tiny_skia::Pixmap::new(side, side).unwrap();
                let t1 = Instant::now();
                anim.render_into(0, &mut pixmap);
                let first = t1.elapsed();
                let mut prev: Vec<u8> = Vec::new();
                let (mut raw_total, mut packed_total) = (0usize, 0usize);
                for k in 0..anim.frame_count() {
                    pixmap.fill(tdr_lottie::tiny_skia::Color::TRANSPARENT);
                    anim.render_into(k, &mut pixmap);
                    let cur = pixmap.data().to_vec();
                    raw_total += cur.len();
                    packed_total += packed_size(&cur, (k % 8 != 0).then_some(&prev[..]));
                    prev = cur;
                }
                println!("  frames {raw_total} B raw, {packed_total} B packed");
                println!(
                    "tgs {} B: ip {} op {} fr {} -> {} frames {:.0} ms; parse {parsed:?}, first frame {first:?}, all {:?}",
                    bytes.len(),
                    anim.in_point,
                    anim.out_point,
                    anim.frame_rate,
                    anim.frame_count(),
                    anim.frame_count() as f64 * 1000. / anim.frame_rate,
                    t1.elapsed()
                );
            }
            EmojiFormat::Webm => {
                let video = tdr_webm::demux(bytes);
                match tdr_webm::decode(bytes, 200) {
                    Ok(d) => println!(
                        "webm {} B {}x{}: blocks {} container {} ms, decoded {} frames {} ms in {:?}",
                        bytes.len(),
                        d.width,
                        d.height,
                        video.as_ref().map_or(0, |v| v.blocks.len()),
                        video.as_ref().map_or(0, |v| v.duration_ms),
                        d.frames.len(),
                        d.frames.iter().map(|f| f.duration_ms).sum::<u32>(),
                        t.elapsed()
                    ),
                    Err(e) => println!("webm: {e}"),
                }
            }
            EmojiFormat::Webp => println!("webp {} B", bytes.len()),
        }
    }
    Ok(())
}

/// The size of a frame packed like the app's custom emoji frames (XOR the previous
/// frame, zero runs with varint counts).
fn packed_size(cur: &[u8], prev: Option<&[u8]>) -> usize {
    let byte = |i: usize| cur[i] ^ prev.map_or(0, |p| p[i]);
    let var = |v: usize| (usize::BITS - v.max(1).leading_zeros()).div_ceil(7) as usize;
    let (n, mut i, mut size) = (cur.len(), 0, 0);
    while i < n {
        let start = i;
        while i < n && byte(i) == 0 {
            i += 1;
        }
        let lit = i;
        while i < n && !(byte(i) == 0 && i + 2 < n && byte(i + 1) == 0 && byte(i + 2) == 0) {
            i += 1;
        }
        size += var(lit - start) + var(i - lit) + (i - lit);
    }
    size
}

/// The custom emoji of the last message in Saved Messages (ids and the Lottie
/// features they use, no text): their files go to `dir` for contact sheets.
async fn probe_saved_emoji(dir: &str) -> Result<()> {
    use std::io::Read;
    let client = logged_in()?;
    let r = client
        .invoke(&functions::messages::GetHistory {
            peer: enums::InputPeer::InputPeerSelf,
            offset_id: 0,
            offset_date: 0,
            add_offset: 0,
            limit: 1,
            max_id: 0,
            min_id: 0,
            hash: 0,
        })
        .await?;
    let messages = match r {
        enums::messages::Messages::Messages(m) => m.messages,
        enums::messages::Messages::Slice(m) => m.messages,
        enums::messages::Messages::ChannelMessages(m) => m.messages,
        enums::messages::Messages::NotModified(_) => vec![],
    };
    let mut ids: Vec<i64> = Vec::new();
    for m in &messages {
        if let enums::Message::Message(m) = m {
            for e in m.entities.iter().flatten() {
                if let enums::MessageEntity::CustomEmoji(e) = e
                    && !ids.contains(&e.document_id)
                {
                    ids.push(e.document_id);
                }
            }
        }
    }
    println!("custom emoji in the last saved message: {}", ids.len());
    let docs = tdr_data::emoji_sets::custom_emoji_documents(&client, &ids).await?;
    std::fs::create_dir_all(dir)?;
    for raw in docs {
        let Some(doc) = tdr_data::emoji_sets::emoji_document(&raw) else {
            continue;
        };
        let bytes = tdr_data::emoji_sets::download_document(&client, &doc).await?;
        let ext = match doc.format {
            tdr_data::emoji_sets::EmojiFormat::Tgs => "tgs",
            tdr_data::emoji_sets::EmojiFormat::Webm => "webm",
            tdr_data::emoji_sets::EmojiFormat::Webp => "webp",
        };
        std::fs::write(format!("{dir}/{}.{ext}", doc.id), &bytes)?;
        if ext != "tgs" {
            println!("{} {ext}", doc.id);
            continue;
        }
        let mut json = String::new();
        flate2::read::GzDecoder::new(&bytes[..]).read_to_string(&mut json)?;
        let v: serde_json::Value = serde_json::from_str(&json)?;
        let mut features = std::collections::BTreeMap::<String, usize>::new();
        fn walk(
            v: &serde_json::Value,
            f: &mut std::collections::BTreeMap<String, usize>,
            in_layer: bool,
        ) {
            match v {
                serde_json::Value::Object(o) => {
                    if in_layer && o.contains_key("ty") && o.contains_key("ip") {
                        *f.entry(format!("layer ty {}", o["ty"])).or_default() += 1;
                        for k in [
                            "tt", "tp", "td", "hasMask", "ef", "sy", "ao", "ct", "hd", "sr", "tm",
                            "parent",
                        ] {
                            if o.contains_key(k) {
                                *f.entry(format!("layer.{k}={}", o[k])).or_default() += 1;
                            }
                        }
                        if let Some(b) = o.get("bm")
                            && b != 0
                        {
                            *f.entry(format!("layer.bm={b}")).or_default() += 1;
                        }
                        if let Some(st) = o.get("st")
                            && st != 0
                        {
                            *f.entry("layer.st!=0".into()).or_default() += 1;
                        }
                        if let Some(m) = o.get("masksProperties").and_then(|m| m.as_array()) {
                            for m in m {
                                *f.entry(format!(
                                    "mask mode {} inv {}",
                                    m["mode"],
                                    m.get("inv").unwrap_or(&serde_json::Value::Null)
                                ))
                                .or_default() += 1;
                            }
                        }
                    }
                    if let Some(ty) = o.get("ty").and_then(|t| t.as_str()) {
                        *f.entry(format!("shape {ty}")).or_default() += 1;
                        if o.get("hd") == Some(&serde_json::Value::Bool(true)) {
                            *f.entry(format!("shape {ty} hidden")).or_default() += 1;
                        }
                        if ty == "tm" && o.contains_key("m") {
                            *f.entry(format!("tm.m={}", o["m"])).or_default() += 1;
                        }
                        if (ty == "fl" || ty == "gf") && o.contains_key("r") {
                            *f.entry(format!("{ty}.r={}", o["r"])).or_default() += 1;
                        }
                        if let Some(b) = o.get("bm")
                            && b != 0
                        {
                            *f.entry(format!("{ty}.bm={b}")).or_default() += 1;
                        }
                        if ty == "gr" && o.contains_key("np") {
                            // fine
                        }
                    }
                    for (k, x) in o {
                        walk(x, f, in_layer || k == "layers");
                    }
                }
                serde_json::Value::Array(a) => {
                    for x in a {
                        walk(x, f, in_layer);
                    }
                }
                _ => {}
            }
        }
        walk(&v, &mut features, false);
        println!(
            "{} tgs fr {} ip {} op {} w {} h {} assets {}",
            doc.id,
            v["fr"],
            v["ip"],
            v["op"],
            v["w"],
            v["h"],
            v["assets"].as_array().map_or(0, |a| a.len())
        );
        for (k, n) in features {
            println!("    {k}: {n}");
        }
    }
    Ok(())
}

async fn probe_folders() -> Result<()> {
    use tdr_data::describe::PeerKind as K;
    use tdr_data::filters::{ChatTraits, FilterEntry};
    let client = logged_in()?;
    let mut sync = Sync::new(client.clone(), open_cache()?)?;
    let list = sync.dialog_filters().await?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as i64;
    let store = sync.store();
    let mut chats: Vec<ChatTraits> = Vec::new();
    for folder in [0, 1] {
        for d in store.dialogs(folder, 100_000)? {
            let info = d
                .peer
                .as_ref()
                .map(tdr_data::describe::peer_info)
                .unwrap_or_default();
            let kind = match d.dialog.peer.kind {
                tdr_data::tdr_storage::PeerKind::Chat => K::Group,
                _ => info.kind,
            };
            chats.push(ChatTraits {
                peer: d.dialog.peer,
                kind,
                contact: info.contact,
                muted: d.dialog.mute_until.is_some_and(|m| i64::from(m) > now),
                unread: d.dialog.unread_count > 0,
                archived: d.dialog.folder_id == 1,
            });
        }
    }
    println!(
        "{} cached chats; unread (all chats): {:?}",
        chats.len(),
        tdr_data::filters::unread_badge(chats.iter(), None)
    );
    let archive = store.dialogs(1, 100_000)?;
    println!(
        "archive: {} chats, {} unread chats, {} unread messages",
        archive.len(),
        archive.iter().filter(|d| d.dialog.unread_count > 0).count(),
        archive.iter().map(|d| d.dialog.unread_count).sum::<i32>()
    );
    let main = store.dialogs(0, 100_000)?;
    println!(
        "main list: {} unread chats, {} unread messages",
        main.iter().filter(|d| d.dialog.unread_count > 0).count(),
        main.iter().map(|d| d.dialog.unread_count).sum::<i32>()
    );
    for (i, entry) in list.iter().enumerate() {
        match entry {
            FilterEntry::All => println!("{i}: All chats"),
            FilterEntry::Folder(f) => println!(
                "{i}: folder id {}, icon {:?}, pinned {}, included {}, excluded {}, holds {} chats, badge {:?}",
                f.id,
                f.icon(),
                f.pinned.len(),
                f.include.len(),
                f.exclude.len(),
                chats.iter().filter(|c| f.contains(c)).count(),
                tdr_data::filters::unread_badge(chats.iter(), Some(f)),
            ),
        }
    }
    Ok(())
}

/// The formatting entities of the latest cached Saved Messages: ids, kinds, UTF-16
/// offsets and lengths only (no texts, no links).
fn probe_entities() -> Result<()> {
    use tdr_data::describe::EntityKind as K;
    let store = open_cache()?;
    let me = store.meta_i64("self_id")?.context("no self id")?;
    let h = store.history(PeerKey::user(me), 30)?;
    for m in &h.messages {
        let info = tdr_data::describe::message_info(m);
        if info.entities.is_empty() {
            continue;
        }
        let units = m.text.encode_utf16().count();
        let kinds: Vec<String> = info
            .entities
            .iter()
            .map(|e| {
                let kind = match &e.kind {
                    K::TextUrl(_) => "TextUrl".to_string(),
                    K::Pre { language } => format!("Pre(lang {} chars)", language.chars().count()),
                    K::MentionName(_) => "MentionName".to_string(),
                    K::FormattedDate { flags, .. } => format!("Date(flags {})", flags.any()),
                    k => format!("{k:?}"),
                };
                format!("{kind}@{}+{}", e.offset, e.length)
            })
            .collect();
        println!("msg {} ({units} units): {}", m.id, kinds.join(" "));
        if let Some((_, raw)) = &m.raw
            && let Ok(tdr_tl::enums::Message::Message(full)) =
                <tdr_tl::enums::Message as tdr_tl::Deserializable>::from_bytes(
                    &tdr_tl::Bytes::copy_from_slice(raw),
                )
        {
            let all: Vec<String> = full
                .entities
                .iter()
                .flatten()
                .map(|e| format!("{e:?}").split('(').next().unwrap_or("").to_string())
                .collect();
            println!(
                "  raw kinds: {} media: {}",
                all.join(" "),
                full.media.is_some()
            );
        }
    }
    Ok(())
}

/// Whether `messages.getPeerDialogs` has a dialog for user `id` (its access hash from
/// the cache; no names printed, nothing written).
async fn probe_peer_dialog(id: i64) -> Result<()> {
    let store = open_cache()?;
    let hash = store
        .access_hash(PeerKey::user(id))?
        .context("no access hash cached")?;
    let client = logged_in()?;
    let enums::messages::PeerDialogs::PeerDialogs(r) = client
        .invoke(&functions::messages::GetPeerDialogs {
            peers: vec![enums::InputDialogPeer::InputDialogPeer(Box::new(
                types::InputDialogPeer {
                    peer: enums::InputPeer::User(types::InputPeerUser {
                        user_id: id,
                        access_hash: hash,
                    }),
                },
            ))],
        })
        .await?;
    for d in &r.dialogs {
        if let enums::Dialog::Dialog(d) = d {
            println!(
                "dialog: top_message {}, folder {:?}, pinned {}, unread {}",
                d.top_message, d.folder_id, d.pinned, d.unread_count
            );
        }
    }
    println!("{} dialogs, {} messages", r.dialogs.len(), r.messages.len());
    Ok(())
}

/// The client's own steps for a chat outside the server's chat list, on the real
/// cache: `keep` = start + the dialog entry a "go to original" asks for; `relist` = a
/// later start + the whole chat list paged (its clean-ups). Prints whether the chat
/// is still listed (ids and flags only).
async fn probe_keep(id: i64, open: bool) -> Result<()> {
    let peer = PeerKey::user(id);
    let client = logged_in()?;
    let mut sync = Sync::new(client.clone(), open_cache()?)?;
    sync.start().await?;
    if open {
        let came = sync.request_dialog_entry(peer).await?;
        println!("dialog entry requested: {came}");
    } else {
        let mut pages = 0;
        while !sync.dialogs_complete() && pages < 50 {
            sync.load_more_dialogs().await?;
            pages += 1;
        }
        println!("chat list paged to its end ({pages} more pages)");
    }
    save_session(&client)?;
    let listed = sync
        .store()
        .dialogs(0, usize::MAX >> 1)?
        .iter()
        .position(|d| d.dialog.peer == peer);
    println!("listed at: {listed:?}");
    Ok(())
}

async fn dialogs(limit: i32) -> Result<()> {
    let started = Instant::now();
    let store = open_cache()?;
    let cached = store.dialogs(0, limit as usize)?;
    if !cached.is_empty() {
        print_dialogs(&cached);
        eprintln!(
            "\n{} chats from the cache in {}",
            cached.len(),
            ms(started.elapsed())
        );
    }

    let client = logged_in()?;
    let mut sync = Sync::new(client.clone(), store)?;
    let t = Instant::now();
    let applied = sync.start().await?;
    save_session(&client)?;
    eprintln!(
        "sync with the server: {} in {}",
        describe(&applied),
        ms(t.elapsed())
    );
    if cached.is_empty()
        || applied.reloaded
        || !applied.new_messages.is_empty()
        || applied.deleted > 0
    {
        let list = sync.store().dialogs(0, limit as usize)?;
        println!();
        print_dialogs(&list);
    }
    Ok(())
}

fn sender_name(store: &Storage, m: &MessageRow) -> String {
    if m.out {
        return "you".into();
    }
    match m.from {
        Some(from) if from != m.peer => store
            .peer(from)
            .ok()
            .flatten()
            .map_or_else(|| format!("{from:?}"), |p| p.name),
        _ => String::new(),
    }
}

fn print_history(store: &Storage, messages: &[MessageRow]) {
    for m in messages.iter().rev() {
        let time = (m.date.rem_euclid(86_400) / 60) as u32;
        let from = sender_name(store, m);
        let from = if from.is_empty() {
            String::new()
        } else {
            format!("{from}: ")
        };
        println!(
            "[{:02}:{:02} UTC] {from}{}",
            time / 60,
            time % 60,
            truncate(&m.text, 100)
        );
    }
}

async fn history(index: usize, count: usize) -> Result<()> {
    let started = Instant::now();
    let store = open_cache()?;
    let list = store.dialogs(0, index.max(1))?;
    let Some(dialog) = index.checked_sub(1).and_then(|i| list.get(i)) else {
        bail!("no chat number {index} in the cached list; run `tdr dialogs` first");
    };
    let peer = dialog.dialog.peer;
    let cached = store.history(peer, count)?;
    if !cached.messages.is_empty() {
        print_history(&store, &cached.messages);
        eprintln!(
            "
{} messages from the cache in {}",
            cached.messages.len(),
            ms(started.elapsed())
        );
    }

    let client = logged_in()?;
    let mut sync = Sync::new(client.clone(), store)?;
    let t = Instant::now();
    let applied = sync.start().await?;
    eprintln!("sync: {} in {}", describe(&applied), ms(t.elapsed()));
    let t = Instant::now();
    let h = sync.history(peer, count).await?;
    save_session(&client)?;
    eprintln!(
        "history: {} messages ({} from the cache, {} loaded from the server) in {}",
        h.messages.len(),
        h.from_cache,
        h.fetched,
        ms(t.elapsed())
    );
    if h.messages.len() != cached.messages.len()
        || h.messages.first().map(|m| m.id) != cached.messages.first().map(|m| m.id)
    {
        println!();
        print_history(sync.store(), &h.messages);
    }
    Ok(())
}

fn open_files() -> Result<FileCache> {
    Ok(FileCache::open(
        data_dir().join("files"),
        FileCacheConfig::default(),
    )?)
}

fn files_info() -> Result<()> {
    let s = open_files()?.stats()?;
    println!(
        "{} files, {:.1} MB (limit {} MB)",
        s.files,
        s.bytes as f64 / 1048576.0,
        FileCacheConfig::default().size_limit / 1048576
    );
    Ok(())
}

fn files_clear() -> Result<()> {
    open_files()?.clear()?;
    println!("File cache cleared.");
    Ok(())
}

/// Prints new messages as they arrive. `seconds`: stop after that long (for tests).
async fn watch(seconds: Option<u64>) -> Result<()> {
    let client = logged_in()?;
    let mut sync = Sync::new(client.clone(), open_cache()?)?;
    let t = Instant::now();
    let applied = sync.start().await?;
    save_session(&client)?;
    eprintln!("caught up: {} in {}", describe(&applied), ms(t.elapsed()));
    eprintln!("waiting for messages (Ctrl+C to stop)…");
    let until = seconds.map(|s| Instant::now() + Duration::from_secs(s));
    loop {
        if until.is_some_and(|u| Instant::now() >= u) {
            break;
        }
        let applied = tokio::select! {
            r = sync.next(Duration::from_secs(1)) => r?,
            _ = tokio::signal::ctrl_c() => break,
        };
        for m in &applied.new_messages {
            let chat = sync
                .store()
                .peer(m.peer)?
                .map_or_else(|| "?".into(), |p| p.name);
            let from = sender_name(sync.store(), m);
            let from = if from.is_empty() {
                String::new()
            } else {
                format!("{from}: ")
            };
            println!("[{}] {from}{}", truncate(&chat, 30), truncate(&m.text, 80));
        }
        if applied.edited + applied.deleted > 0 || applied.reloaded {
            eprintln!("  ({})", describe(&applied));
        }
    }
    save_session(&client)?;
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
    let mut files = open_files()?;
    let key = format!("avatar:{}:big", photo.photo_id);
    let t_cache = Instant::now();
    if let Some(data) = files.get(&key)? {
        let path = data_dir().join("avatar.jpg");
        std::fs::write(&path, &data)?;
        println!(
            "Saved {} KB to {} (from the file cache)",
            data.len() / 1024,
            path.display()
        );
        eprintln!(
            "\nfile cache hit: {}; total with getUsers: {}",
            ms(t_cache.elapsed()),
            ms(t.elapsed())
        );
        return Ok(());
    }
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
    files.put(&key, &data)?;
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
