//! The Accounts page: sign in to GitHub, GitLab and Gitea with OAuth, add SSH keys to a
//! signed-in account, and add a self-hosted server with its own OAuth app's client ID.
//! What a row says lives in `mantisgit_core::auth`; this draws it and runs the work off
//! the UI thread.

use super::*;
use mantisgit_core::auth::{self, Account, Host, flow};
use mantisgit_core::ops::Cancel;
use mantisgit_core::repo::forge::Forge;

pub(super) struct State {
    hosts: Vec<Host>,
    accounts: Vec<Account>,
    /// Private keys with a `.pub` beside them, which can be added to an account.
    keys: Vec<PathBuf>,
    /// The server a sign-in is waiting on, and how to stop it.
    signing_in: Option<(String, Cancel)>,
    /// The key being added, as `origin key`, so its button can say so.
    adding_key: Option<String>,
    loaded: bool,
    /// Whether the keychain has said who is signed in.
    accounts_read: bool,
    /// Addresses of the servers added by hand, which can be edited and removed.
    custom: Vec<String>,
    /// The address of the server the form is changing, when it isn't adding a new one.
    editing: Option<String>,
    new_origin: Entity<InputState>,
    new_client_id: Entity<InputState>,
    new_forge: Forge,
}

impl State {
    pub(super) fn new(
        window: &mut Window,
        cx: &mut Context<Settings>,
    ) -> (Self, Vec<Subscription>) {
        let new_origin = field("https://git.example.com", "", window, cx);
        let new_client_id = field("Client ID of the OAuth app", "", window, cx);
        let mut events = Vec::new();
        for input in [&new_origin, &new_client_id] {
            events.push(cx.subscribe_in(
                input,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.save_host(window, cx);
                    }
                    cx.notify();
                },
            ));
        }
        let state = Self {
            hosts: Vec::new(),
            accounts: Vec::new(),
            keys: Vec::new(),
            signing_in: None,
            adding_key: None,
            loaded: false,
            accounts_read: false,
            custom: Vec::new(),
            editing: None,
            new_origin,
            new_client_id,
            new_forge: Forge::Gitea,
        };
        (state, events)
    }

    fn account_on(&self, host: &Host) -> Option<&Account> {
        self.accounts.iter().find(|a| a.host.origin == host.origin)
    }
}

impl Settings {
    /// Reads the servers and the local keys, which draws the page at once, then who is
    /// signed in. That is a keychain call, which can wait on a locked or missing keyring
    /// service, so the page must not wait for it.
    pub(super) fn load_accounts(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let (hosts, keys, custom) = cx
                .background_spawn(async {
                    let hosts = auth::hosts();
                    let custom = auth::custom_hosts().into_iter().map(|h| h.origin).collect();
                    (hosts, ssh::keys(), custom)
                })
                .await;
            let known = hosts.clone();
            this.update(cx, |this, cx| {
                let state = &mut this.accounts;
                (state.hosts, state.keys, state.custom) = (hosts, keys, custom);
                state.loaded = true;
                state.accounts_read = false;
                cx.notify();
            })
            .ok();
            let accounts = cx
                .background_spawn(async move { auth::accounts_in(&auth::token::Keychain, &known) })
                .await;
            this.update(cx, |this, cx| {
                this.accounts.accounts = accounts;
                this.accounts.accounts_read = true;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Opens the browser at the server and waits for the person to come back.
    fn sign_in(&mut self, host: Host, cx: &mut Context<Self>) {
        if self.accounts.signing_in.is_some() {
            return;
        }
        // Listening starts here, so the address is ready before the browser opens.
        let pending = match auth::begin_sign_in(&host) {
            Ok(pending) => pending,
            Err(message) => {
                self.feedback = Feedback::Failed(message);
                cx.notify();
                return;
            }
        };
        cx.open_url(&pending.url);
        let cancel = Cancel::new();
        self.accounts.signing_in = Some((host.origin.clone(), cancel.clone()));
        self.feedback = Feedback::Saved(format!(
            "Finish signing in to {} in your browser.",
            host.name()
        ));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let asked = host.clone();
            let outcome = cx
                .background_spawn(async move { auth::finish_sign_in(&host, pending, &cancel) })
                .await;
            this.update(cx, |this, cx| {
                this.accounts.signing_in = None;
                this.feedback = match outcome {
                    Ok(account) => {
                        this.accounts
                            .accounts
                            .retain(|a| a.host.origin != asked.origin);
                        let said = format!("Signed in to {} as {}.", asked.name(), account.login);
                        this.accounts.accounts.push(account);
                        Feedback::Saved(said)
                    }
                    Err(message) => Feedback::Failed(message),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn cancel_sign_in(&mut self, cx: &mut Context<Self>) {
        if let Some((_, cancel)) = &self.accounts.signing_in {
            cancel.cancel();
        }
        cx.notify();
    }

    fn sign_out(&mut self, host: Host, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let asked = host.clone();
            let outcome = cx
                .background_spawn(async move { auth::sign_out(&host) })
                .await;
            this.update(cx, |this, cx| {
                this.feedback = match outcome {
                    Ok(()) => {
                        this.accounts
                            .accounts
                            .retain(|a| a.host.origin != asked.origin);
                        Feedback::Saved(auth::signed_out_message(&asked))
                    }
                    Err(message) => Feedback::Failed(message),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Adds the public key beside `key` to the account on `host`.
    fn upload_key(&mut self, host: Host, key: PathBuf, cx: &mut Context<Self>) {
        if self.accounts.adding_key.is_some() {
            return;
        }
        let id = format!("{} {}", host.origin, key.display());
        self.accounts.adding_key = Some(id);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_spawn(async move {
                    let public = PathBuf::from(format!("{}.pub", key.display()));
                    auth::upload_ssh_key(&host, &public, &auth::machine_name())
                })
                .await;
            this.update(cx, |this, cx| {
                this.accounts.adding_key = None;
                this.feedback = match outcome {
                    Ok(said) => Feedback::Saved(said),
                    Err(message) => Feedback::Failed(message),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Fills the form with a server added by hand, to change it.
    fn edit_host(&mut self, host: &Host, window: &mut Window, cx: &mut Context<Self>) {
        let state = &mut self.accounts;
        state.editing = Some(host.origin.clone());
        state.new_forge = host.forge;
        let (origin, id) = (host.origin.clone(), host.client_id.clone());
        state
            .new_origin
            .update(cx, |input, cx| input.set_value(origin, window, cx));
        state
            .new_client_id
            .update(cx, |input, cx| input.set_value(id, window, cx));
        cx.notify();
    }

    fn clear_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = &mut self.accounts;
        state.editing = None;
        for input in [&state.new_origin, &state.new_client_id] {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
        cx.notify();
    }

    /// Adds the server in the form, or saves the changes to the one being edited.
    fn save_host(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let origin = self.accounts.new_origin.read(cx).value().to_string();
        let client_id = self.accounts.new_client_id.read(cx).value().to_string();
        let Some(host) = Host::new(self.accounts.new_forge, &origin, &client_id) else {
            self.feedback = Feedback::Failed(
                "That isn't a server address. Write it as https://git.example.com.".into(),
            );
            cx.notify();
            return;
        };
        if client_id.trim().is_empty() {
            self.feedback =
                Feedback::Failed("Paste the client ID of the OAuth app you made there.".into());
            cx.notify();
            return;
        }
        let replacing = self.accounts.editing.clone();
        cx.spawn_in(window, async move |this, cx| {
            let saved = {
                let (replacing, host) = (replacing.clone(), host.clone());
                cx.background_spawn(async move { auth::save_host(replacing.as_deref(), &host) })
                    .await
            };
            this.update_in(cx, |this, window, cx| {
                match saved {
                    Ok(()) => {
                        this.feedback = Feedback::Saved(match replacing {
                            Some(_) => format!("Saved {}.", host.name()),
                            None => format!(
                                "Added {}. The OAuth app's redirect address must be {}.",
                                host.name(),
                                flow::REDIRECT
                            ),
                        });
                        this.clear_form(window, cx);
                        this.load_accounts(cx);
                    }
                    Err(message) => this.feedback = Feedback::Failed(message),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Forgets a server added by hand and its token.
    fn remove_host(&mut self, host: Host, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let origin = host.origin.clone();
            let removed = {
                let origin = origin.clone();
                cx.background_spawn(async move { auth::remove_host(&origin) })
                    .await
            };
            this.update_in(cx, |this, window, cx| {
                match removed {
                    Ok(()) => {
                        this.feedback = Feedback::Saved(format!(
                            "Removed {}. Its token is gone from this computer.",
                            host.name()
                        ));
                        if this.accounts.editing.as_deref() == Some(origin.as_str()) {
                            this.clear_form(window, cx);
                        }
                        this.load_accounts(cx);
                    }
                    Err(message) => this.feedback = Feedback::Failed(message),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn render_host_row(&self, index: usize, host: &Host, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let account = self.accounts.account_on(host);
        let waiting = self
            .accounts
            .signing_in
            .as_ref()
            .is_some_and(|(origin, _)| *origin == host.origin);
        let busy = self.accounts.signing_in.is_some();
        let custom = self.accounts.custom.contains(&host.origin);
        let sentence = if waiting {
            "Waiting for the browser…".to_owned()
        } else if !self.accounts.accounts_read {
            "Asking the system keychain…".to_owned()
        } else {
            auth::status_sentence(host, account)
        };
        let mark = host.forge.name();
        let button = if waiting {
            Button::new(("sign-in", index))
                .ghost()
                .small()
                .label("Cancel")
                .on_click(cx.listener(|this, _, _, cx| this.cancel_sign_in(cx)))
        } else {
            let target = host.clone();
            let signed_in = account.is_some();
            Button::new(("sign-in", index))
                .ghost()
                .small()
                .label(auth::action_label(host, account))
                .disabled(
                    busy || !self.accounts.accounts_read || (!signed_in && !host.can_sign_in()),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if signed_in {
                        this.sign_out(target.clone(), cx);
                    } else {
                        this.sign_in(target.clone(), cx);
                    }
                }))
        };
        div()
            .px_4()
            .py_3()
            .gap_3()
            .h_flex()
            .child(
                div()
                    .v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        div()
                            .text_size(TEXT_BODY)
                            .line_height(px(20.))
                            .child(format!("{} · {mark}", host.name())),
                    )
                    .child(
                        div()
                            .text_size(TEXT_SMALL)
                            .line_height(px(17.))
                            .text_color(muted)
                            .child(sentence),
                    ),
            )
            .child(
                div()
                    .h_flex()
                    .gap_1()
                    .child(button)
                    .children(custom.then(|| {
                        let (edit, remove) = (host.clone(), host.clone());
                        div()
                            .h_flex()
                            .gap_1()
                            .child(
                                Button::new(("edit-host", index))
                                    .ghost()
                                    .small()
                                    .label("Edit")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.edit_host(&edit, window, cx)
                                    })),
                            )
                            .child(
                                Button::new(("remove-host", index))
                                    .ghost()
                                    .small()
                                    .label("Remove")
                                    .disabled(busy)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.remove_host(remove.clone(), window, cx)
                                    })),
                            )
                    })),
            )
            .into_any_element()
    }

    /// One row for each key, with a button for each account it can be added to.
    fn render_key_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let muted = cx.theme().muted_foreground;
        let mut rows = Vec::new();
        for (key_index, key) in self.accounts.keys.iter().enumerate() {
            let buttons = self
                .accounts
                .accounts
                .iter()
                .enumerate()
                .map(|(i, account)| {
                    let (host, path) = (account.host.clone(), key.clone());
                    let id = format!("{} {}", host.origin, key.display());
                    let adding = self.accounts.adding_key.as_deref() == Some(id.as_str());
                    let label = if adding {
                        "Adding…".to_owned()
                    } else {
                        format!("Add to {}", host.name())
                    };
                    Button::new(("add-key", key_index * 16 + i))
                        .ghost()
                        .small()
                        .label(label)
                        .disabled(self.accounts.adding_key.is_some())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.upload_key(host.clone(), path.clone(), cx)
                        }))
                });
            rows.push(
                div()
                    .px_4()
                    .py_3()
                    .gap_3()
                    .h_flex()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(TEXT_BODY)
                            .line_height(px(20.))
                            .child(ssh::display_key(key)),
                    )
                    .child(div().h_flex().gap_1().children(buttons))
                    .into_any_element(),
            );
        }
        if rows.is_empty() {
            rows.push(
                div()
                    .px_4()
                    .py_3()
                    .text_size(TEXT_SMALL)
                    .text_color(muted)
                    .child("No SSH keys in ~/.ssh.")
                    .into_any_element(),
            );
        }
        rows
    }

    fn render_add_host(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let picked = self.accounts.new_forge;
        let chips = Forge::ALL.into_iter().enumerate().map(|(i, forge)| {
            chip(("new-host-forge", i), forge.name(), picked == forge, cx).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.accounts.new_forge = forge;
                    cx.notify();
                },
            ))
        });
        let labelled = |text: &'static str, input: &Entity<InputState>| {
            div()
                .v_flex()
                .gap_1()
                .child(section_label(muted).child(text))
                .child(Input::new(input))
        };
        div()
            .v_flex()
            .px_4()
            .py_3()
            .gap_3()
            .child(
                div()
                    .text_size(TEXT_SMALL)
                    .line_height(px(17.))
                    .text_color(muted)
                    .child(
                        "For a server your company runs. Make an OAuth application there, \
                         not confidential, with the redirect address http://127.0.0.1/callback, and \
                         paste its client ID.",
                    ),
            )
            .child(div().flex().flex_wrap().gap_1().children(chips))
            .child(labelled("Server", &self.accounts.new_origin))
            .child(labelled("Client ID", &self.accounts.new_client_id))
            .child(
                div()
                    .h_flex()
                    .gap_1()
                    .child(
                        Button::new("add-host")
                            .ghost()
                            .small()
                            .label(match &self.accounts.editing {
                                Some(origin) => format!(
                                    "Save {}",
                                    origin.split_once("://").map_or(origin.as_str(), |p| p.1)
                                ),
                                None => "Add server".to_owned(),
                            })
                            .on_click(
                                cx.listener(|this, _, window, cx| this.save_host(window, cx)),
                            ),
                    )
                    .children(self.accounts.editing.is_some().then(|| {
                        Button::new("cancel-host")
                            .ghost()
                            .small()
                            .label("Cancel")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.clear_form(window, cx)),
                            )
                    })),
            )
            .into_any_element()
    }

    pub(super) fn render_accounts(&self, cx: &mut Context<Self>) -> AnyElement {
        let sentence = "Sign in with OAuth to use HTTPS remotes without a password, and to \
                        add SSH keys to your account. Tokens are kept in the system keychain.";
        let hosts: Vec<AnyElement> = self
            .accounts
            .hosts
            .iter()
            .enumerate()
            .map(|(i, host)| self.render_host_row(i, host, cx))
            .collect();
        let mut page = div()
            .v_flex()
            .gap_6()
            .w_full()
            .max_w(PAGE_WIDTH)
            .child(self.page_head("Accounts", sentence, cx));
        if !self.accounts.loaded {
            return page.into_any_element();
        }
        page = page.child(self.render_group("Servers", hosts, cx));
        if !self.accounts.accounts.is_empty() {
            let rows = self.render_key_rows(cx);
            page = page.child(self.render_group("SSH keys", rows, cx));
        }
        page.child(self.render_group(
            if self.accounts.editing.is_some() {
                "Edit server"
            } else {
                "Another server"
            },
            vec![self.render_add_host(cx)],
            cx,
        ))
        .into_any_element()
    }
}
