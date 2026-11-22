//! The window's root: the view of the shown account, made anew when another account
//! is shown (tdesktop rebuilds its `SessionController` the same way).

use std::path::PathBuf;

use gpui::{AppContext, Context, Entity, Focusable, IntoElement, Render, Subscription, Window};

use crate::accounts::{Accounts, Activated};
use crate::bridge::Command;
use crate::ui::MainView;

pub struct Root {
    accounts: Entity<Accounts>,
    view: Entity<MainView>,
    columns_path: PathBuf,
    _focus_lost: Subscription,
}

impl Root {
    pub fn new(
        accounts: Entity<Accounts>,
        dark: bool,
        columns_path: PathBuf,
        open_on_start: Option<usize>,
        show_intro: Option<bool>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let view = Self::make_view(&accounts, dark, &columns_path, window, cx);
        view.update(cx, |v, cx| {
            v.open_on_start = open_on_start;
            // `Some(can_back)`: the login screens over the account (demo).
            if let Some(can_back) = show_intro {
                v.show_intro(can_back, cx);
            }
        });
        cx.subscribe_in(&accounts, window, |this, _, _: &Activated, window, cx| {
            this.rebuild(window, cx);
        })
        .detach();
        // The focused element went away (a chat opened from a link, a menu closed):
        // keys would reach only the window's root, past the view's Esc and other
        // window-wide actions. The view takes the focus back, as tdesktop's window
        // always handles Esc.
        let focus_lost = cx.on_focus_lost(window, |this, window, cx| {
            window.focus(&this.view.focus_handle(cx));
        });
        Self {
            accounts,
            view,
            columns_path,
            _focus_lost: focus_lost,
        }
    }

    fn make_view(
        accounts: &Entity<Accounts>,
        dark: bool,
        columns_path: &std::path::Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<MainView> {
        let commands = accounts.read(cx).active().map(|s| s.commands.clone());
        let logged_in = commands.is_some();
        // No account yet: the login screens, with nothing to talk to behind them.
        let commands = commands.unwrap_or_else(|| tokio::sync::mpsc::unbounded_channel().0);
        let accounts_for_view = accounts.clone();
        let columns = columns_path.to_path_buf();
        let view = cx.new(|cx| {
            let mut v = MainView::new(commands, accounts_for_view, dark, cx);
            v.set_columns_path(columns);
            if !logged_in {
                v.show_intro(false, cx);
            }
            v
        });
        accounts.update(cx, |a, _| a.set_view(view.downgrade()));
        window.focus(&view.focus_handle(cx));
        view
    }

    /// Another account is shown: a fresh view, and its data thread sends all again.
    fn rebuild(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dark = self.view.read(cx).is_dark();
        self.view = Self::make_view(&self.accounts, dark, &self.columns_path, window, cx);
        if let Some(slot) = self.accounts.read(cx).active() {
            let _ = slot.commands.send(Command::Refresh);
        }
        cx.notify();
    }

    pub fn window_activation_changed(&mut self, active: bool, cx: &mut Context<Self>) {
        self.view
            .update(cx, |v, _| v.window_activation_changed(active));
    }
}

impl Render for Root {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.view.clone()
    }
}
