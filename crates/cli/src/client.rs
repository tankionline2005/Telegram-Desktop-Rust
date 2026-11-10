use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tdr_crypto::AuthKey;
use tdr_mtproto::{Error, PRODUCTION_DCS, Sender, Update};
use tdr_tl::{RemoteCall, functions};
use tokio::sync::{Mutex, mpsc};

use crate::session_file::{self, SavedSession};

pub fn credentials() -> Result<(i32, &'static str)> {
    let (Some(id), Some(hash)) = (option_env!("TDR_API_ID"), option_env!("TDR_API_HASH")) else {
        bail!(
            "API credentials are missing: copy .env.example to .env, fill in api_id/api_hash \
             from https://my.telegram.org and rebuild"
        );
    };
    Ok((id.parse().context("TDR_API_ID must be a number")?, hash))
}

fn dc_addr(dc_id: i32) -> Result<&'static str> {
    PRODUCTION_DCS
        .iter()
        .find(|(id, _)| *id == dc_id)
        .map(|(_, a)| *a)
        .with_context(|| format!("unknown DC {dc_id}"))
}

pub struct Client {
    sender: Sender,
    updates: Mutex<mpsc::UnboundedReceiver<Update>>,
    dc_id: i32,
    auth_key: AuthKey,
    time_offset: i32,
    /// `initConnection` must wrap the first request on every connection.
    initialized: AtomicBool,
}

impl Client {
    async fn connect(dc_id: i32, key: Option<(AuthKey, i32)>) -> Result<Self> {
        let c = Sender::connect(dc_addr(dc_id)?, dc_id, key, false).await?;
        Ok(Self {
            sender: c.sender,
            updates: Mutex::new(c.updates),
            dc_id,
            auth_key: c.auth_key,
            time_offset: c.time_offset,
            initialized: AtomicBool::new(false),
        })
    }

    /// Connects with a brand-new auth key (used for login).
    pub async fn connect_new(dc_id: i32) -> Result<Self> {
        Self::connect(dc_id, None).await
    }

    /// Connects with the saved session.
    pub async fn connect_saved() -> Result<Self> {
        let s = session_file::load()?.context("not logged in; run `tdr login` first")?;
        Self::connect(s.dc_id, Some((s.auth_key, s.time_offset))).await
    }

    pub fn save(&self) -> Result<()> {
        session_file::save(&SavedSession {
            dc_id: self.dc_id,
            auth_key: self.auth_key.clone(),
            time_offset: self.time_offset,
        })
    }

    pub async fn invoke<R: RemoteCall>(&self, request: R) -> Result<R::Return, Error> {
        if self.initialized.swap(true, Ordering::AcqRel) {
            return self.sender.invoke(&request).await;
        }
        let (api_id, _) =
            credentials().map_err(|_| Error::Security("API credentials are missing"))?;
        let wrapped = functions::InvokeWithLayer {
            layer: tdr_tl::LAYER,
            query: functions::InitConnection {
                api_id,
                device_model: "Desktop".into(),
                system_version: std::env::consts::OS.into(),
                app_version: env!("CARGO_PKG_VERSION").into(),
                system_lang_code: "en".into(),
                lang_pack: String::new(),
                lang_code: "en".into(),
                proxy: None,
                params: None,
                query: request,
            },
        };
        let result = self.sender.invoke(&wrapped).await;
        if result.is_err() {
            self.initialized.store(false, Ordering::Release);
        }
        result
    }

    /// Waits for the next update or the timeout.
    pub async fn wait_update(&self, timeout: Duration) {
        let mut rx = self.updates.lock().await;
        let _ = tokio::time::timeout(timeout, rx.recv()).await;
    }
}
