//! The Accounts page: sign in to GitHub, GitLab and Gitea with OAuth, add SSH keys to a
//! signed-in account, and set up the OAuth app of any server, public or self-hosted, in a
//! form that opens under its row.
//! What a row says lives in `mantisgit_core::auth`; this draws it and runs the work off
//! the UI thread.

use super::*;
use mantisgit_core::auth::{self, Account, Host, flow};
use mantisgit_core::ops::Cancel;
use mantisgit_core::repo::forge::Forge;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long the keychain has to say who is signed in.
const KEYCHAIN_PATIENCE: Duration = Duration::from_secs(5);

/// What the server form is doing.
#[derive(Clone, PartialEq, Eq)]
enum Form {
    /// Adding a server that isn't listed yet.
    Add,
    /// Changing the app of the server listed with this address.
    Edit(Host),
}

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
    /// The keychain didn't answer in time, so who is signed in is unknown.
    keychain_silent: bool,
    /// Addresses of the servers with an app set up here, which can be removed (or, for a
    /// public server, put back to the app this build came with).
    own: Vec<String>,
    form: Option<Form>,
    /// The saved secret goes when the form is saved.
    drop_secret: bool,
    new_origin: Entity<InputState>,
    new_client_id: Entity<InputState>,
    new_secret: Entity<InputState>,
    new_forge: Forge,
}

impl State {
    pub(super) fn new(
        window: &mut Window,
        cx: &mut Context<Settings>,
    ) -> (Self, Vec<Subscription>) {
        let new_origin = field("https://git.example.com", "", window, cx);
        let new_client_id = field("Application ID", "", window, cx);
        let new_secret = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder(NO_SECRET);
            input.set_masked(true, window, cx);
            input
        });
        let mut events = Vec::new();
        for input in [&new_origin, &new_client_id, &new_secret] {
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
            keychain_silent: false,
            own: Vec::new(),
            form: None,
            drop_secret: false,
            new_origin,
            new_client_id,
            new_secret,
            new_forge: Forge::GitLab,
        };
        (state, events)
    }

    fn account_on(&self, host: &Host) -> Option<&Account> {
        self.accounts.iter().find(|a| a.host.origin == host.origin)
    }

    fn editing(&self, host: &Host) -> bool {
        matches!(&self.form, Some(Form::Edit(edited)) if edited.origin == host.origin)
    }
}

/// What the secret input says when there is no secret to keep.
const NO_SECRET: &str = "Leave empty for a public app";
const KEPT_SECRET: &str = "Kept in the system keychain. Type a new one to replace it.";
const DROPPED_SECRET: &str = "Removed when you save";

impl Settings {
    /// Reads the servers and the local keys, which draws the page at once, then who is
    /// signed in. That is a keychain call, which can wait on a locked or missing keyring
    /// service, so the page must not wait for it.
    pub(super) fn load_accounts(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let (hosts, keys, own) = cx
                .background_spawn(async { (auth::hosts(), ssh::keys(), auth::own_apps()) })
                .await;
            let known = hosts.clone();
            this.update(cx, |this, cx| {
                let state = &mut this.accounts;
                (state.hosts, state.keys, state.own) = (hosts, keys, own);
                state.loaded = true;
                state.accounts_read = false;
                cx.notify();
            })
            .ok();
            // The keychain can wait forever on a locked keyring, and a blocked call can't
            // be cancelled, so it runs on a thread of its own and is given a few seconds.
            let slot = Arc::new(Mutex::new(None));
            let reply = slot.clone();
            std::thread::spawn(move || {
                let accounts = auth::accounts_in(&auth::token::Keychain, &known);
                if let Ok(mut slot) = reply.lock() {
                    *slot = Some(accounts);
                }
            });
            let mut accounts = None;
            for _ in 0..KEYCHAIN_PATIENCE.as_millis() / 100 {
                accounts = slot.lock().ok().and_then(|mut slot| slot.take());
                if accounts.is_some() {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
            }
            this.update(cx, |this, cx| {
                this.accounts.keychain_silent = accounts.is_none();
                this.accounts.accounts = accounts.unwrap_or_default();
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

    /// Opens the form under a server's row, filled with its app.
    fn edit_host(&mut self, host: &Host, window: &mut Window, cx: &mut Context<Self>) {
        self.open_form(Form::Edit(host.clone()), window, cx);
    }

    fn open_form(&mut self, form: Form, window: &mut Window, cx: &mut Context<Self>) {
        let (origin, id, confidential) = match &form {
            Form::Edit(host) => (
                host.origin.clone(),
                host.client_id.clone(),
                host.confidential,
            ),
            Form::Add => (String::new(), String::new(), false),
        };
        let state = &mut self.accounts;
        if let Form::Edit(host) = &form {
            state.new_forge = host.forge;
        }
        state.form = Some(form);
        state.drop_secret = false;
        state
            .new_origin
            .update(cx, |input, cx| input.set_value(origin, window, cx));
        state
            .new_client_id
            .update(cx, |input, cx| input.set_value(id, window, cx));
        state.new_secret.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.set_placeholder(
                if confidential { KEPT_SECRET } else { NO_SECRET },
                window,
                cx,
            );
        });
        cx.notify();
    }

    fn clear_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = &mut self.accounts;
        state.form = None;
        state.drop_secret = false;
        for input in [&state.new_origin, &state.new_client_id, &state.new_secret] {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
        cx.notify();
    }

    /// Marks the saved secret to go, or keeps it after all.
    fn toggle_drop_secret(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = &mut self.accounts;
        state.drop_secret = !state.drop_secret;
        let placeholder = if state.drop_secret {
            DROPPED_SECRET
        } else {
            KEPT_SECRET
        };
        state.new_secret.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.set_placeholder(placeholder, window, cx);
        });
        cx.notify();
    }

    fn copy_redirect(&mut self, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(flow::REDIRECT.to_owned()));
        self.feedback = Feedback::Saved(format!("Copied {}", flow::REDIRECT));
        cx.notify();
    }

    /// Adds the server in the form, or saves the app of the one being edited.
    fn save_host(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = self.accounts.form.clone() else {
            return;
        };
        let client_id = self.accounts.new_client_id.read(cx).value().to_string();
        let typed_secret = self.accounts.new_secret.read(cx).value().trim().to_string();
        // A public server keeps its address and forge; an added one can change both.
        let (forge, origin, replacing) = match &form {
            Form::Edit(host) if host.is_public() => {
                (host.forge, host.origin.clone(), Some(host.origin.clone()))
            }
            Form::Edit(host) => (
                self.accounts.new_forge,
                self.accounts.new_origin.read(cx).value().to_string(),
                Some(host.origin.clone()),
            ),
            Form::Add => (
                self.accounts.new_forge,
                self.accounts.new_origin.read(cx).value().to_string(),
                None,
            ),
        };
        let Some(host) = Host::new(forge, &origin, &client_id) else {
            self.feedback = Feedback::Failed(
                "That isn't a server address. Write it as https://git.example.com.".into(),
            );
            cx.notify();
            return;
        };
        if client_id.trim().is_empty() {
            self.feedback = Feedback::Failed(format!(
                "Paste the client ID (application ID) of the OAuth app you made on {}.",
                host.name()
            ));
            cx.notify();
            return;
        }
        let secret = if !typed_secret.is_empty() {
            auth::Secret::Set(typed_secret)
        } else if self.accounts.drop_secret {
            auth::Secret::Clear
        } else {
            auth::Secret::Keep
        };
        cx.spawn_in(window, async move |this, cx| {
            let saved = {
                let (replacing, host) = (replacing.clone(), host.clone());
                cx.background_spawn(
                    async move { auth::save_host(replacing.as_deref(), &host, secret) },
                )
                .await
            };
            this.update_in(cx, |this, window, cx| {
                match saved {
                    Ok(()) => {
                        this.feedback = Feedback::Saved(match replacing {
                            Some(_) => format!("Saved the app on {}.", host.name()),
                            None => format!("Added {}.", host.name()),
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

    /// Forgets the app set up for a server, its secret and its token.
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
                        this.feedback = Feedback::Saved(if host.is_public() {
                            format!(
                                "Forgot your app on {}. Its token and secret are gone from this \
                                 computer.",
                                host.name()
                            )
                        } else {
                            format!(
                                "Removed {}. Its token and secret are gone from this computer.",
                                host.name()
                            )
                        });
                        if this.accounts.editing(&host) {
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
        let own = self.accounts.own.contains(&host.origin);
        let editing = self.accounts.editing(host);
        let sentence = if waiting {
            "Waiting for the browser…".to_owned()
        } else if !self.accounts.accounts_read {
            "Asking the system keychain…".to_owned()
        } else if self.accounts.keychain_silent && account.is_none() {
            "The system keychain didn't answer, so this can't say. Is a keyring service \
             running and unlocked? Open this page again to retry."
                .to_owned()
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
                    .child(
                        Button::new(("edit-host", index))
                            .ghost()
                            .small()
                            .label("Edit")
                            .disabled(busy || editing)
                            .on_click({
                                let edit = host.clone();
                                cx.listener(move |this, _, window, cx| {
                                    this.edit_host(&edit, window, cx)
                                })
                            }),
                    )
                    .children(own.then(|| {
                        let remove = host.clone();
                        Button::new(("remove-host", index))
                            .ghost()
                            .small()
                            .label(if host.is_public() {
                                "Forget app"
                            } else {
                                "Remove"
                            })
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.remove_host(remove.clone(), window, cx)
                            }))
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

    /// The form that sets up a server's OAuth app, under its row or at the foot of the
    /// card when adding one.
    fn render_form(&self, form: &Form, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, code_bg, border) = (theme.muted_foreground, theme.background, theme.border);
        let state = &self.accounts;
        // A public server's address and forge are fixed: only its app changes.
        let fixed = match form {
            Form::Edit(host) if host.is_public() => Some(host),
            _ => None,
        };
        let forge = fixed.map_or(state.new_forge, |host| host.forge);
        let server = match form {
            Form::Edit(host) => host.name().to_owned(),
            Form::Add => auth::host::origin_of(&state.new_origin.read(cx).value())
                .and_then(|origin| origin.split_once("://").map(|(_, name)| name.to_owned()))
                .unwrap_or_else(|| "the server".to_owned()),
        };
        let confidential = matches!(form, Form::Edit(host) if host.confidential);
        let labelled = |text: &'static str, input: &Entity<InputState>| {
            div()
                .v_flex()
                .gap_1()
                .child(section_label(muted).child(text))
                .child(Input::new(input))
        };
        let chips = Forge::ALL.into_iter().enumerate().map(|(i, choice)| {
            chip(("new-host-forge", i), choice.name(), forge == choice, cx).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.accounts.new_forge = choice;
                    cx.notify();
                },
            ))
        });
        let redirect = div()
            .v_flex()
            .gap_1()
            .child(section_label(muted).child("Redirect address"))
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .px_2()
                            .py_1()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(border)
                            .bg(code_bg)
                            .text_size(TEXT_SMALL)
                            .child(flow::REDIRECT),
                    )
                    .child(
                        Button::new("copy-redirect")
                            .ghost()
                            .small()
                            .label("Copy")
                            .on_click(cx.listener(|this, _, _, cx| this.copy_redirect(cx))),
                    ),
            );
        let secret_label = div()
            .h_flex()
            .gap_2()
            .child(
                section_label(muted)
                    .flex_1()
                    .child("Client secret, only for a confidential app"),
            )
            .children(confidential.then(|| {
                Button::new("drop-secret")
                    .ghost()
                    .small()
                    .label(if state.drop_secret {
                        "Keep secret"
                    } else {
                        "Remove secret"
                    })
                    .on_click(
                        cx.listener(|this, _, window, cx| this.toggle_drop_secret(window, cx)),
                    )
            }));
        let save_label = match form {
            Form::Edit(host) => format!("Save {}", host.name()),
            Form::Add => "Add server".to_owned(),
        };
        div()
            .v_flex()
            .px_4()
            .py_3()
            .gap_3()
            .child(section_label(muted).child(match form {
                Form::Edit(host) => format!("Your OAuth app on {}", host.name()),
                Form::Add => "Add a server".to_owned(),
            }))
            .when(fixed.is_none(), |form| {
                form.child(div().flex().flex_wrap().gap_1().children(chips))
                    .child(labelled("Server", &state.new_origin))
            })
            .child(
                div()
                    .text_size(TEXT_SMALL)
                    .line_height(px(17.))
                    .text_color(muted)
                    .child(Host::setup_steps(forge, &server)),
            )
            .child(redirect)
            .child(labelled("Client ID", &state.new_client_id))
            .child(
                div()
                    .v_flex()
                    .gap_1()
                    .child(secret_label)
                    .child(Input::new(&state.new_secret)),
            )
            .child(
                div()
                    .h_flex()
                    .gap_1()
                    .child(
                        Button::new("save-host")
                            .ghost()
                            .small()
                            .label(save_label)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.save_host(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("cancel-host")
                            .ghost()
                            .small()
                            .label("Cancel")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.clear_form(window, cx)),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// The foot of the Servers card: "+ Add server", or the form when it is open.
    fn render_add_row(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.accounts.form == Some(Form::Add) {
            return self.render_form(&Form::Add, cx);
        }
        let link = cx.theme().primary;
        div()
            .px_4()
            .py_3()
            .h_flex()
            .child(
                div()
                    .id("add-host")
                    .px_1()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .text_size(TEXT_BODY)
                    .text_color(link)
                    .font_semibold()
                    .child("+ Add server")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.open_form(Form::Add, window, cx)),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_accounts(&self, cx: &mut Context<Self>) -> AnyElement {
        let sentence = "Sign in with OAuth to use HTTPS remotes without a password, and to \
                        add SSH keys to your account. Each server needs an OAuth app: Edit \
                        sets it up. Tokens and secrets are kept in the system keychain.";
        let mut rows = Vec::new();
        for (i, host) in self.accounts.hosts.iter().enumerate() {
            rows.push(self.render_host_row(i, host, cx));
            if self.accounts.editing(host) {
                let form = Form::Edit(host.clone());
                rows.push(self.render_form(&form, cx));
            }
        }
        rows.push(self.render_add_row(cx));
        let mut page = div()
            .v_flex()
            .gap_6()
            .w_full()
            .max_w(PAGE_WIDTH)
            .child(self.page_head("Accounts", sentence, cx));
        if !self.accounts.loaded {
            return page.into_any_element();
        }
        page = page.child(self.render_group("Servers", rows, cx));
        if !self.accounts.accounts.is_empty() {
            let rows = self.render_key_rows(cx);
            page = page.child(self.render_group("SSH keys", rows, cx));
        }
        page.into_any_element()
    }
}
