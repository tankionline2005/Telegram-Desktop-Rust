//! The window's root: the view of the shown account, made anew when another account
//! is shown (tdesktop rebuilds its `SessionController` the same way).

use std::path::PathBuf;

use gpui::{AppContext, Context, Entity, Focusable, IntoElement, Render, Window};

use crate::accounts::{Accounts, Activated};
use crate::bridge::Command;
use crate::ui::MainView;

pub struct Root {
    accounts: Entity<Accounts>,
    view: Entity<MainView>,
    columns_path: PathBuf,
}

impl Root {
    pub fn new(
        accounts: Entity<Accounts>,
        dark: bool,
        columns_path: PathBuf,
        open_on_start: Option<usize>,
        show_intro: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let view = Self::make_view(&accounts, dark, &columns_path, window, cx);
        view.update(cx, |v, cx| {
            v.open_on_start = open_on_start;
            if show_intro {
                v.show_intro(true, cx);
            }
        });
        cx.subscribe_in(&accounts, window, |this, _, _: &Activated, window, cx| {
            this.rebuild(window, cx);
        })
        .detach();
        Self {
            accounts,
            view,
            columns_path,
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
