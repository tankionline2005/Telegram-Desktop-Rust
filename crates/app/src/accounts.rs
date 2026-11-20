//! Several accounts in one window (tdesktop's `Main::Domain`).
//!
//! Every logged-in account has its own data thread (connection, cache, updates), as
//! in tdesktop, where all sessions stay alive: the menu shows each one's unread
//! counter and the menu button a dot for the others. The window shows one of them:
//! its updates go to the [`MainView`]; the others only feed their row in the menu.
//!
//! The first account keeps the data folder itself (`data/`), the others live in
//! `data/accounts/<id>/`; `data/accounts.txt` keeps their order and the active one.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{Context, EventEmitter, RenderImage, WeakEntity};
use tdr_api::{Config, SessionData};
use tokio::sync::mpsc;

use crate::bridge::{self, AccountState, Command, MeInfo, Update};
use crate::ui::MainView;

/// `Main::Domain::kMaxAccounts`, `kPremiumMaxAccounts`.
pub const MAX_ACCOUNTS: usize = 3;
pub const PREMIUM_MAX_ACCOUNTS: usize = 6;

const REGISTRY_FILE: &str = "accounts.txt";

/// The accounts' order, the active one and whether the menu shows the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registry {
    pub ids: Vec<u32>,
    pub active: u32,
    /// `Core::Settings::mainMenuAccountsShown`.
    pub menu_accounts: bool,
}

impl Registry {
    pub fn load(base: &Path) -> Self {
        let text = std::fs::read_to_string(base.join(REGISTRY_FILE)).unwrap_or_default();
        let mut registry = Self {
            ids: Vec::new(),
            active: 1,
            menu_accounts: false,
        };
        for line in text.lines() {
            let (key, value) = line.split_once('=').unwrap_or((line, ""));
            match key.trim() {
                "order" => {
                    registry.ids = value
                        .split(',')
                        .filter_map(|v| v.trim().parse().ok())
                        .collect();
                }
                "active" => registry.active = value.trim().parse().unwrap_or(1),
                "menu_accounts" => registry.menu_accounts = value.trim() == "1",
                _ => {}
            }
        }
        if registry.ids.is_empty() {
            registry.ids.push(1);
        }
        registry
    }

    pub fn save(&self, base: &Path) {
        let order: Vec<String> = self.ids.iter().map(u32::to_string).collect();
        let text = format!(
            "order={}\nactive={}\nmenu_accounts={}\n",
            order.join(","),
            self.active,
            u8::from(self.menu_accounts)
        );
        let _ = std::fs::create_dir_all(base);
        let _ = tdr_api::write_atomic(&base.join(REGISTRY_FILE), text.as_bytes());
    }

    /// An id for a new account.
    pub fn next_id(&self) -> u32 {
        self.ids.iter().copied().max().unwrap_or(0) + 1
    }
}

/// The folder of account `id`.
pub fn account_dir(base: &Path, id: u32) -> PathBuf {
    if id == 1 {
        base.to_path_buf()
    } else {
        base.join("accounts").join(id.to_string())
    }
}

pub fn paths(base: &Path, id: u32) -> bridge::Paths {
    let dir = account_dir(base, id);
    bridge::Paths {
        session: dir.join("session.bin"),
        cache: dir.join("cache.sqlite"),
        files: dir.join("files"),
    }
}

/// The account has a saved, logged-in session.
pub fn logged_in(base: &Path, id: u32) -> bool {
    std::fs::read(paths(base, id).session)
        .ok()
        .and_then(|d| SessionData::from_bytes(&d).ok())
        .is_some_and(|s| s.is_authorized())
}

/// A running account.
pub struct AccountSlot {
    pub id: u32,
    pub commands: mpsc::UnboundedSender<Command>,
    pub me: Option<MeInfo>,
    /// Its own userpic, with the photo id.
    pub userpic: Option<(i64, Arc<RenderImage>)>,
    pub state: AccountState,
    asked_photo: Option<i64>,
}

/// The window should show the active account anew.
pub struct Activated;

pub struct Accounts {
    base: PathBuf,
    config: Option<Config>,
    demo: bool,
    pub registry: Registry,
    pub slots: Vec<AccountSlot>,
    /// The view the active account's updates go to.
    view: Option<WeakEntity<MainView>>,
}

impl EventEmitter<Activated> for Accounts {}

impl Accounts {
    /// Starts the data threads of the logged-in accounts (`demo`: that many fake
    /// accounts).
    pub fn new(
        base: PathBuf,
        config: Option<Config>,
        demo: Option<usize>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut registry = Registry::load(&base);
        let mut this = Self {
            base: base.clone(),
            config,
            demo: demo.is_some(),
            registry: registry.clone(),
            slots: Vec::new(),
            view: None,
        };
        match demo {
            Some(count) => {
                registry.ids = (1..=count.max(1) as u32).collect();
                registry.active = 1;
                for id in registry.ids.clone() {
                    this.start(id, cx);
                }
            }
            None => {
                registry.ids.retain(|id| logged_in(&base, *id));
                for id in registry.ids.clone() {
                    this.start(id, cx);
                }
                if !registry.ids.contains(&registry.active) {
                    registry.active = registry.ids.first().copied().unwrap_or(1);
                }
            }
        }
        if registry.ids.len() < 2 {
            registry.menu_accounts = false;
        }
        this.registry = registry;
        this
    }

    /// The data thread of account `id` and the task routing its updates.
    fn start(&mut self, id: u32, cx: &mut Context<Self>) {
        let (commands, mut updates) = match (&self.config, self.demo) {
            (_, true) => bridge::start_demo(id as usize - 1),
            (Some(config), false) => bridge::start(config.clone(), paths(&self.base, id)),
            (None, false) => return,
        };
        self.slots.push(AccountSlot {
            id,
            commands,
            me: None,
            userpic: None,
            state: AccountState::default(),
            asked_photo: None,
        });
        cx.spawn(async move |this, cx| {
            while let Some(update) = updates.recv().await {
                if this.update(cx, |a, cx| a.route(id, update, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn route(&mut self, id: u32, update: Update, cx: &mut Context<Self>) {
        let active = self.registry.active == id;
        if let Some(slot) = self.slots.iter_mut().find(|s| s.id == id) {
            match &update {
                Update::Me(me) => {
                    if let Some((photo_id, dc_id)) = me.photo
                        && slot.asked_photo != Some(photo_id)
                    {
                        slot.asked_photo = Some(photo_id);
                        let _ = slot.commands.send(Command::LoadUserpic {
                            peer: me.peer,
                            photo_id,
                            dc_id,
                        });
                    }
                    slot.me = Some(me.clone());
                    cx.notify();
                }
                Update::AccountState(state) => {
                    slot.state = *state;
                    cx.notify();
                }
                Update::Userpic {
                    peer,
                    photo_id,
                    image,
                } if slot.me.as_ref().is_some_and(|m| m.peer == *peer) => {
                    slot.userpic = Some((*photo_id, image.clone()));
                    cx.notify();
                }
                _ => {}
            }
        }
        if active && let Some(view) = self.view.as_ref().and_then(|v| v.upgrade()) {
            view.update(cx, |v, cx| v.apply(update, cx));
        }
    }

    pub fn set_view(&mut self, view: WeakEntity<MainView>) {
        self.view = Some(view);
    }

    pub fn is_demo(&self) -> bool {
        self.demo
    }

    pub fn config(&self) -> Option<&Config> {
        self.config.as_ref()
    }

    pub fn base(&self) -> &Path {
        &self.base
    }

    pub fn active(&self) -> Option<&AccountSlot> {
        self.slots.iter().find(|s| s.id == self.registry.active)
    }

    /// The accounts in the menu's order.
    pub fn ordered(&self) -> Vec<&AccountSlot> {
        self.registry
            .ids
            .iter()
            .filter_map(|id| self.slots.iter().find(|s| s.id == *id))
            .collect()
    }

    /// `OtherAccountsUnreadState`: unread messages of the other accounts, and whether
    /// all of them are muted.
    pub fn others_unread(&self) -> (u32, bool) {
        let others = self.slots.iter().filter(|s| s.id != self.registry.active);
        let (mut count, mut all_muted) = (0u32, true);
        for s in others {
            if s.state.unread > 0 {
                count += s.state.unread;
                all_muted &= s.state.unread_muted;
            }
        }
        (count, count > 0 && all_muted)
    }

    /// How many accounts there may be: 6 with Premium on any of them, else 3.
    pub fn max_accounts(&self) -> usize {
        if self
            .slots
            .iter()
            .any(|s| s.me.as_ref().is_some_and(|m| m.premium))
        {
            PREMIUM_MAX_ACCOUNTS
        } else {
            MAX_ACCOUNTS
        }
    }

    pub fn toggle_menu_accounts(&mut self, cx: &mut Context<Self>) {
        self.registry.menu_accounts = !self.registry.menu_accounts;
        self.save();
        cx.notify();
    }

    fn save(&self) {
        if !self.demo {
            self.registry.save(&self.base);
        }
    }

    /// Shows account `id` (`Main::Domain::activate`).
    pub fn activate(&mut self, id: u32, cx: &mut Context<Self>) {
        if self.registry.active == id || !self.slots.iter().any(|s| s.id == id) {
            return;
        }
        self.registry.active = id;
        self.save();
        cx.emit(Activated);
        cx.notify();
    }

    /// The id the account being added will get, with its session file.
    pub fn new_account(&self) -> (u32, PathBuf) {
        let id = if self.registry.ids.is_empty() && !logged_in(&self.base, 1) {
            1
        } else {
            self.registry.next_id()
        };
        (id, paths(&self.base, id).session)
    }

    /// A login finished: the account joins the list and is shown.
    pub fn logged_in(&mut self, id: u32, cx: &mut Context<Self>) {
        if !self.registry.ids.contains(&id) {
            self.registry.ids.push(id);
        }
        self.start(id, cx);
        self.registry.active = id;
        self.save();
        cx.emit(Activated);
        cx.notify();
    }
}

/// One login in its own thread (`tdr_data::login::run`).
pub fn start_login(
    config: Config,
    session_path: PathBuf,
) -> (
    mpsc::UnboundedSender<tdr_data::login::LoginCommand>,
    mpsc::UnboundedReceiver<tdr_data::login::LoginEvent>,
) {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (ev_tx, ev_rx) = mpsc::unbounded_channel();
    std::thread::Builder::new()
        .name("login".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(async move {
                let client = tdr_api::Client::new(config, SessionData::new(2));
                tdr_data::login::run(client, session_path, cmd_rx, ev_tx).await;
            });
        })
        .expect("spawn login thread");
    (cmd_tx, ev_rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_roundtrip() {
        let dir = std::env::temp_dir().join(format!("tdr-accounts-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let r = Registry {
            ids: vec![1, 3, 2],
            active: 3,
            menu_accounts: true,
        };
        r.save(&dir);
        assert_eq!(Registry::load(&dir), r);
        assert_eq!(r.next_id(), 4);
        assert_eq!(account_dir(&dir, 1), dir);
        assert_eq!(account_dir(&dir, 2), dir.join("accounts").join("2"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
