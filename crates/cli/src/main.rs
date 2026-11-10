//! `tdr` — console client for exercising the protocol stack.
//!
//! ```text
//! tdr ping            connect to a DC with a fresh key and measure each step
//! tdr login           log in by QR code (and cloud password, if set)
//! tdr me              show the logged-in account
//! tdr dialogs [N]     list the first N chats
//! tdr logout          terminate the session
//! ```

mod client;
mod session_file;

use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use qrcode::QrCode;
use qrcode::render::unicode;
use tdr_mtproto::RpcError;
use tdr_tl::{enums, functions, types};

use crate::client::Client;

const USAGE: &str = "\
usage:
  tdr ping            connect to Telegram with a fresh key and time each step
  tdr login           log in by scanning a QR code (Settings → Devices → Link Desktop Device)
  tdr me              show the logged-in account
  tdr dialogs [N]     list the first N chats (default 20)
  tdr logout          log out and delete the local session";

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
        ["login"] => login().await,
        ["me"] => me().await,
        ["dialogs"] => dialogs(20).await,
        ["dialogs", n] => dialogs(n.parse().context("N must be a number")?).await,
        ["logout"] => logout().await,
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
    let t = Instant::now();
    let client = Client::connect_new(2).await?;
    println!("TCP + auth key (DH, RSA_PAD): {}", ms(t.elapsed()));

    let t = Instant::now();
    let enums::NearestDc::NearestDc(dc) = client.invoke(functions::help::GetNearestDc {}).await?;
    println!(
        "First RPC (initConnection + help.getNearestDc): {}",
        ms(t.elapsed())
    );
    println!(
        "  country {}, this DC {}, nearest DC {}",
        dc.country, dc.this_dc, dc.nearest_dc
    );

    let t = Instant::now();
    let enums::Config::Config(cfg) = client.invoke(functions::help::GetConfig {}).await?;
    println!(
        "help.getConfig: {} ({} DC options)",
        ms(t.elapsed()),
        cfg.dc_options.len()
    );

    let t = Instant::now();
    let n = 20;
    let client = std::sync::Arc::new(client);
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..n {
        let c = client.clone();
        tasks.spawn(async move { c.invoke(functions::help::GetNearestDc {}).await });
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

async fn login() -> Result<()> {
    if session_file::load()?.is_some() {
        bail!("already logged in; run `tdr logout` first");
    }
    let mut client = Client::connect_new(2).await?;
    let (api_id, api_hash) = client::credentials()?;
    let mut shown_token = Vec::new();

    let authorization = loop {
        let result = client
            .invoke(functions::auth::ExportLoginToken {
                api_id,
                api_hash: api_hash.to_string(),
                except_ids: vec![],
            })
            .await;
        let token = match result {
            Ok(t) => t,
            Err(tdr_mtproto::Error::Rpc(e)) if e.is("SESSION_PASSWORD_NEEDED") => {
                break check_password(&client).await?;
            }
            Err(e) => return Err(e.into()),
        };
        match token {
            enums::auth::LoginToken::LoginToken(t) => {
                if t.token[..] != shown_token[..] {
                    print_qr(&t.token)?;
                    shown_token = t.token.to_vec();
                }
                // Wait for `updateLoginToken` (any update wakes us up) or re-check soon.
                client.wait_update(Duration::from_secs(5)).await;
            }
            enums::auth::LoginToken::MigrateTo(m) => {
                client = Client::connect_new(m.dc_id).await?;
                match client
                    .invoke(functions::auth::ImportLoginToken { token: m.token })
                    .await
                {
                    Ok(enums::auth::LoginToken::Success(s)) => break s.authorization,
                    Ok(_) => bail!("unexpected answer to auth.importLoginToken"),
                    Err(tdr_mtproto::Error::Rpc(e)) if e.is("SESSION_PASSWORD_NEEDED") => {
                        break check_password(&client).await?;
                    }
                    Err(e) => return Err(e.into()),
                }
            }
            enums::auth::LoginToken::Success(s) => break s.authorization,
        }
    };

    client.save()?;
    match authorization {
        enums::auth::Authorization::Authorization(a) => {
            println!("\nLogged in as {}.", user_name(&a.user))
        }
        enums::auth::Authorization::SignUpRequired(_) => bail!("this phone number has no account"),
    }
    Ok(())
}

async fn check_password(client: &Client) -> Result<enums::auth::Authorization> {
    let enums::account::Password::Password(pw) =
        client.invoke(functions::account::GetPassword {}).await?;
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
        let answer = {
            let (s1, s2, pw) = (
                algo.salt1.clone(),
                algo.salt2.clone(),
                password.into_bytes(),
            );
            let hash =
                tokio::task::spawn_blocking(move || tdr_crypto::srp::password_hash(&pw, &s1, &s2))
                    .await?;
            tdr_crypto::srp::compute_with_hash(&params, &hash)
                .context("bad SRP parameters from server")?
        };
        let check = functions::auth::CheckPassword {
            password: types::InputCheckPasswordSRP {
                srp_id,
                a: answer.a.into(),
                m1: answer.m1.into(),
            }
            .into(),
        };
        match client.invoke(check).await {
            Ok(auth) => return Ok(auth),
            Err(tdr_mtproto::Error::Rpc(e)) if e.is("PASSWORD_HASH_INVALID") => {
                println!("Wrong password, try again.");
            }
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

async fn me() -> Result<()> {
    let client = Client::connect_saved().await?;
    let users = client
        .invoke(functions::users::GetUsers {
            id: vec![enums::InputUser::InputUserSelf],
        })
        .await?;
    for u in &users {
        println!("{}", user_name(u));
    }
    Ok(())
}

async fn dialogs(limit: i32) -> Result<()> {
    let client = Client::connect_saved().await?;
    let t = Instant::now();
    let result = client
        .invoke(functions::messages::GetDialogs {
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

async fn logout() -> Result<()> {
    let client = Client::connect_saved().await?;
    match client.invoke(functions::auth::LogOut {}).await {
        Ok(_) | Err(tdr_mtproto::Error::Rpc(RpcError { code: 401, .. })) => {}
        Err(e) => return Err(e.into()),
    }
    session_file::delete()?;
    println!("Logged out.");
    Ok(())
}
