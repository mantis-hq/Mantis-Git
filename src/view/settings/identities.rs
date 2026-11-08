//! The Identities page: who the open repository commits as and why, and the identities
//! with their SSH keys and the rules that pick them.

use super::*;

impl Settings {
    /// Saves what was typed for the default (`None`) or an identity. The label of the
    /// default is fixed.
    pub(super) fn commit_identity(
        &mut self,
        id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(row) = self.identity_rows.iter().find(|row| row.id == id) else {
            return;
        };
        let profile = Profile {
            name: row.name.read(cx).value().trim().to_owned(),
            email: row.email.read(cx).value().trim().to_owned(),
            signing_key: row.key.read(cx).value().trim().to_owned(),
            ssh_key: row.ssh_key.read(cx).value().trim().to_owned(),
        };
        let label = row.label.read(cx).value().trim().to_owned();
        match id {
            // The default's name and email are the global config, edited on the Git
            // config page. Its SSH key is only here.
            None => {
                let key = profile.ssh_key;
                if key == self.book.default.ssh_key {
                    return;
                }
                self.change(
                    "Saved the default's SSH key.".to_owned(),
                    move |_, base| identity::save_default_ssh_key(base, &key),
                    None,
                    window,
                    cx,
                );
            }
            Some(id) => {
                let unchanged = self
                    .book
                    .identity(&id)
                    .is_some_and(|known| known.label == label && known.profile == profile);
                if unchanged {
                    return;
                }
                let saved = format!("Saved {label}.");
                let identity = Identity { id, label, profile };
                self.change(
                    saved,
                    move |_, base| identity::save_identity(base, &identity).map(drop),
                    None,
                    window,
                    cx,
                );
            }
        }
    }

    pub(super) fn add_identity(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let identity = Identity {
            id: String::new(),
            label: self.new_label.read(cx).value().trim().to_owned(),
            profile: Profile {
                name: self.new_identity_name.read(cx).value().trim().to_owned(),
                email: self.new_identity_email.read(cx).value().trim().to_owned(),
                signing_key: self.new_identity_key.read(cx).value().trim().to_owned(),
                ssh_key: self.new_identity_ssh.read(cx).value().trim().to_owned(),
            },
        };
        let saved = format!("Added the identity {}.", identity.label);
        self.change(
            saved,
            move |_, base| identity::save_identity(base, &identity).map(drop),
            Some(|this, window, cx| {
                for input in [
                    &this.new_label,
                    &this.new_identity_name,
                    &this.new_identity_email,
                    &this.new_identity_key,
                    &this.new_identity_ssh,
                ] {
                    input.update(cx, |input, cx| input.set_value("", window, cx));
                }
            }),
            window,
            cx,
        );
    }

    pub(super) fn delete_identity(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let label = self
            .book
            .identity(id)
            .map_or_else(|| id.to_owned(), |identity| identity.label.clone());
        let id = id.to_owned();
        let saved = format!("Removed {label}, and the rules that picked it.");
        self.change(
            saved,
            move |_, base| identity::delete_identity(base, &id),
            None,
            window,
            cx,
        );
    }

    /// Puts `key` in an identity's SSH key, or takes it out if it already is that. Picking
    /// is a decision, so for a saved identity it is saved at once.
    pub(super) fn pick_key(
        &mut self,
        target: &KeyTarget,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = match target {
            KeyTarget::Identity(id) => self
                .identity_rows
                .iter()
                .find(|row| row.id.as_deref() == Some(id))
                .map(|row| row.ssh_key.clone()),
            KeyTarget::Default => self
                .identity_rows
                .iter()
                .find(|row| row.id.is_none())
                .map(|row| row.ssh_key.clone()),
            KeyTarget::New => Some(self.new_identity_ssh.clone()),
        };
        let Some(input) = input else {
            return;
        };
        let next = if input.read(cx).value().trim() == key {
            String::new()
        } else {
            key.to_owned()
        };
        input.update(cx, |input, cx| input.set_value(next, window, cx));
        match target {
            KeyTarget::Identity(id) => self.commit_identity(Some(id.clone()), window, cx),
            KeyTarget::Default => self.commit_identity(None, window, cx),
            KeyTarget::New => cx.notify(),
        }
    }

    pub(super) fn show_rule_form(&mut self, id: &str, shown: bool, cx: &mut Context<Self>) {
        if let Some(row) = self
            .identity_rows
            .iter_mut()
            .find(|row| row.id.as_deref() == Some(id))
        {
            row.adding_rule = shown;
            cx.notify();
        }
    }

    fn set_rule_kind(
        &mut self,
        id: &str,
        kind: NewKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(row) = self
            .identity_rows
            .iter_mut()
            .find(|row| row.id.as_deref() == Some(id))
        {
            row.rule_kind = kind;
            row.rule.update(cx, |input, cx| {
                input.set_placeholder(kind.placeholder(), window, cx)
            });
            cx.notify();
        }
    }

    pub(super) fn add_rule(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self
            .identity_rows
            .iter()
            .find(|row| row.id.as_deref() == Some(id))
        else {
            return;
        };
        let matcher = row
            .rule_kind
            .matcher(row.rule.read(cx).value().trim().to_owned());
        let label = self
            .book
            .identity(id)
            .map_or_else(|| id.to_owned(), |identity| identity.label.clone());
        let saved = format!("{label} is now picked for {}.", matcher.tidy().describe());
        let id = id.to_owned();
        self.change(
            saved,
            move |_, base| identity::add_rule(base, &id, &matcher),
            Some(|this, window, cx| {
                for row in &mut this.identity_rows {
                    row.adding_rule = false;
                    row.rule
                        .update(cx, |input, cx| input.set_value("", window, cx));
                }
            }),
            window,
            cx,
        );
    }

    pub(super) fn remove_rule(
        &mut self,
        matcher: &Match,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let matcher = matcher.clone();
        let saved = format!("No identity is picked for {} any more.", matcher.describe());
        self.change(
            saved,
            move |_, base| identity::remove_rule(base, &matcher),
            None,
            window,
            cx,
        );
    }

    /// Makes the open repository use `choice`.
    pub(super) fn pick_identity(
        &mut self,
        choice: Choice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let name = repository_name(&repo);
        let kept = match &self.repo_state {
            Some(Ok(state)) => state
                .own
                .as_ref()
                .is_some_and(|own| !self.book.has_profile(own)),
            _ => false,
        };
        let mut saved = match &choice {
            Choice::Automatic => format!("{name} now follows the rules."),
            Choice::Identity(id) => {
                let label = self
                    .book
                    .identity(id)
                    .map_or(id.as_str(), |identity| identity.label.as_str());
                format!("{name} now commits as {label}.")
            }
        };
        if kept {
            saved += " Its own name and email were kept as an identity.";
        }
        self.change(
            saved,
            move |_, base| identity::pick(base, &repo, &choice),
            None,
            window,
            cx,
        );
    }

    pub(super) fn render_identities(&self, cx: &mut Context<Self>) -> AnyElement {
        let page = div().v_flex().w_full().max_w(PAGE_WIDTH).gap_6().child(
            self.page_head(
                "Identities",
                "Who you commit as. Make one for each, then pick it for a repository or let rules choose.",
                cx,
            ),
        );
        if let Some(Err(message)) = &self.git {
            return page
                .child(self.render_no_git(message, cx))
                .into_any_element();
        }
        let rows: Vec<AnyElement> = self
            .identity_rows
            .iter()
            .enumerate()
            .map(|(index, row)| self.render_identity(index, row, cx))
            .collect();
        page.child(self.render_picker(cx))
            .child(self.render_group("Your identities", rows, cx))
            .child(self.render_new_identity(cx))
            .into_any_element()
    }

    /// The open repository's identity, and one click to change it.
    fn render_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let quiet = |text: String| {
            div()
                .text_size(TEXT_SMALL)
                .line_height(px(17.))
                .text_color(muted)
                .child(text)
        };
        let Some(repo) = self.repo.as_deref() else {
            return self.render_group(
                "Right now",
                vec![
                    div()
                        .px_4()
                        .py_3()
                        .child(quiet(
                            "Open a repository to choose who it commits as.".to_owned(),
                        ))
                        .into_any_element(),
                ],
                cx,
            );
        };
        let body = match &self.repo_state {
            Some(Ok(state)) => {
                let (own, pinned) = (state.own.as_ref(), state.pinned.as_deref());
                let mut chips = vec![
                    chip(
                        "pick-automatic",
                        "Automatic",
                        own.is_none() && pinned.is_none(),
                        cx,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.pick_identity(Choice::Automatic, window, cx)
                    }))
                    .into_any_element(),
                ];
                for (index, identity) in self.book.identities.iter().enumerate() {
                    let id = identity.id.clone();
                    let on = own.is_none() && pinned == Some(identity.id.as_str());
                    chips.push(
                        chip(("pick-identity", index), &identity.label, on, cx)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.pick_identity(Choice::Identity(id.clone()), window, cx)
                            }))
                            .into_any_element(),
                    );
                }
                if own.is_some() {
                    chips.push(
                        chip("pick-own", "This repository's own", true, cx).into_any_element(),
                    );
                }
                let hint = if own.is_some() {
                    "Set in this repository's own config. Picking another replaces it, and keeps it as an identity."
                } else if pinned.is_some() {
                    "Chosen for this repository only. Automatic goes back to the rules."
                } else {
                    "Rules pick an identity by host or folder. With no match, the Default applies."
                };
                div()
                    .v_flex()
                    .px_4()
                    .py_3()
                    .gap_3()
                    .child(
                        div()
                            .text_size(TEXT_BODY)
                            .line_height(px(20.))
                            .child(state.committing.sentence()),
                    )
                    .child(div().flex().flex_wrap().gap_1().children(chips))
                    .child(quiet(hint.to_owned()))
                    .into_any_element()
            }
            Some(Err(message)) => div()
                .px_4()
                .py_3()
                .child(quiet(message.clone()))
                .into_any_element(),
            None => div()
                .px_4()
                .py_3()
                .child(quiet("Reading…".to_owned()))
                .into_any_element(),
        };
        self.render_group(&repository_name(repo), vec![body], cx)
    }

    /// One identity: its name, email and key, and where it is used automatically.
    fn render_identity(
        &self,
        index: usize,
        row: &IdentityRow,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let id = row.id.clone();
        let labelled = |text: &'static str, input: &Entity<InputState>| {
            div()
                .v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(section_label(muted).child(text))
                .child(Input::new(input))
        };
        let head = match &id {
            None => div()
                .v_flex()
                .flex_1()
                .min_w_0()
                .child(div().text_size(TEXT_BODY).font_semibold().child("Default"))
                .child(
                    div()
                        .text_size(TEXT_SMALL)
                        .line_height(px(17.))
                        .text_color(muted)
                        .child("Your global name and email, used when no rule or choice picks another. Change them on the Git config page. Its SSH key is set here."),
                ),
            Some(id) => {
                let id = id.clone();
                div()
                    .h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_3()
                    .child(div().flex_1().min_w_0().child(Input::new(&row.label)))
                    .child(
                        Button::new(("remove-identity", index))
                            .ghost()
                            .compact()
                            .label("Remove")
                            .tooltip("Delete this identity and its rules")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.delete_identity(&id, window, cx)
                            })),
                    )
            }
        };
        // The default is shown, not edited here: it is the global config, which has its
        // own page, and a card that looks like the others invites changing it by accident.
        let shown = |text: &'static str, value: &str| {
            div()
                .v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(section_label(muted).child(text))
                .child(
                    div()
                        .min_h(px(32.))
                        .truncate()
                        .text_size(TEXT_BODY)
                        .line_height(px(32.))
                        .when(value.is_empty(), |value| value.text_color(muted))
                        .child(if value.is_empty() { "Not set" } else { value }.to_owned()),
                )
        };
        let fields = if id.is_none() {
            let profile = &self.book.default;
            div()
                .h_flex()
                .gap_3()
                .child(shown("Name", &profile.name))
                .child(shown("Email", &profile.email))
                .child(shown("Signing key", &profile.signing_key))
        } else {
            div()
                .h_flex()
                .gap_3()
                .child(labelled("Name", &row.name))
                .child(labelled("Email", &row.email))
                .child(labelled("Signing key", &row.key))
        };
        let card = div()
            .v_flex()
            .px_4()
            .py_3()
            .gap_3()
            .child(div().h_flex().child(head))
            .child(fields);
        match id {
            None => card
                .child(self.render_ssh_key(KeyTarget::Default, index, &row.ssh_key, cx))
                .into_any_element(),
            Some(id) => card
                .child(self.render_ssh_key(
                    KeyTarget::Identity(id.clone()),
                    index,
                    &row.ssh_key,
                    cx,
                ))
                .child(self.render_rules(index, row, &id, cx))
                .into_any_element(),
        }
    }

    /// The SSH key an identity pushes and pulls with: typed as a path, or picked from the
    /// keys found in `~/.ssh`.
    fn render_ssh_key(
        &self,
        target: KeyTarget,
        slot: usize,
        input: &Entity<InputState>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let current = input.read(cx).value().trim().to_owned();
        let keys = self
            .suggestions
            .as_ref()
            .map(|suggestions| suggestions.keys.clone())
            .unwrap_or_default();
        let chips: Vec<AnyElement> = keys
            .into_iter()
            .enumerate()
            .map(|(position, key)| {
                let name = key.rsplit('/').next().unwrap_or(&key).to_owned();
                let on = key == current;
                let target = target.clone();
                chip(("ssh-key", slot * 100 + position), &name, on, cx)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.pick_key(&target, &key, window, cx)
                    }))
                    .into_any_element()
            })
            .collect();
        div()
            .v_flex()
            .gap_1()
            .child(section_label(muted).child("SSH key (optional)"))
            .child(Input::new(input))
            .when(!chips.is_empty(), |area| {
                area.child(div().flex().flex_wrap().gap_1().children(chips))
            })
            .child(
                div()
                    .text_size(TEXT_SMALL)
                    .line_height(px(17.))
                    .text_color(muted)
                    .child(
                        "Fetch, pull and push use this key, and only this one, wherever the identity applies.",
                    ),
            )
            .into_any_element()
    }

    /// Where an identity is picked automatically, and a way to add to that.
    fn render_rules(
        &self,
        index: usize,
        row: &IdentityRow,
        id: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let (muted, selected, quiet) = (
            theme.muted_foreground,
            selected_row(theme),
            theme.secondary_foreground,
        );
        let rules: Vec<Match> = self
            .book
            .rules_for(id)
            .map(|rule| rule.matcher.clone())
            .collect();
        let shown: Vec<AnyElement> = rules
            .iter()
            .enumerate()
            .map(|(position, matcher)| {
                let stop = matcher.clone();
                div()
                    .h_flex()
                    .h(px(26.))
                    .pl_2()
                    .gap_1()
                    .rounded(px(6.))
                    .bg(selected)
                    .text_size(TEXT_SMALL)
                    .text_color(quiet)
                    .child(sentence_case(&matcher.describe()))
                    .child(
                        Button::new(("remove-rule", index * 100 + position))
                            .ghost()
                            .compact()
                            .label("×")
                            .tooltip("Stop picking this identity for that")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.remove_rule(&stop, window, cx)
                            })),
                    )
                    .into_any_element()
            })
            .collect();

        let mut area = div()
            .v_flex()
            .gap_2()
            .child(section_label(muted).child("Picked automatically for"));
        area = if shown.is_empty() {
            area.child(
                div()
                    .text_size(TEXT_SMALL)
                    .line_height(px(17.))
                    .text_color(muted)
                    .child("Nothing yet. Pick it by hand for a repository, or add a rule."),
            )
        } else {
            area.child(div().flex().flex_wrap().gap_1().children(shown))
        };

        if !row.adding_rule {
            let open = id.to_owned();
            return area
                .child(
                    div().h_flex().child(
                        Button::new(("add-rule", index))
                            .ghost()
                            .compact()
                            .label("Add a rule")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.show_rule_form(&open, true, cx)
                            })),
                    ),
                )
                .into_any_element();
        }

        let kinds: Vec<AnyElement> =
            NewKind::ALL
                .into_iter()
                .enumerate()
                .map(|(position, (kind, name))| {
                    let id = id.to_owned();
                    chip(
                        ("rule-kind", index * 10 + position),
                        name,
                        row.rule_kind == kind,
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_rule_kind(&id, kind, window, cx)
                    }))
                    .into_any_element()
                })
                .collect();
        let known: Vec<AnyElement> = match (row.rule_kind, &self.suggestions) {
            (NewKind::Host, Some(info)) => info
                .hosts
                .iter()
                .filter(|host| !rules.contains(&Match::Host((*host).clone())))
                .enumerate()
                .map(|(position, host)| {
                    let pick = host.clone();
                    let input = row.rule.clone();
                    chip(("rule-host", index * 100 + position), host, false, cx)
                        .on_click(cx.listener(move |_, _, window, cx| {
                            let pick = pick.clone();
                            input.update(cx, |input, cx| input.set_value(pick, window, cx));
                        }))
                        .into_any_element()
                })
                .collect(),
            _ => Vec::new(),
        };
        let help = match row.rule_kind {
            NewKind::Host => {
                "The name after @ in a remote like git@github-work:team/app.git, as in your ssh config."
            }
            NewKind::Folder => "Every repository inside this folder.",
            NewKind::Remote => {
                "A pattern for the remote's URL. * stops at a slash and ** after a slash goes on."
            }
        };
        let ready = !row.rule.read(cx).value().trim().is_empty();
        let (add, cancel) = (id.to_owned(), id.to_owned());
        area.child(div().h_flex().gap_1().children(kinds))
            .child(Input::new(&row.rule))
            .child(
                div()
                    .text_size(TEXT_SMALL)
                    .line_height(px(17.))
                    .text_color(muted)
                    .child(help),
            )
            .when(!known.is_empty(), |area| {
                area.child(div().flex().flex_wrap().gap_1().children(known))
            })
            .child(
                div()
                    .h_flex()
                    .justify_end()
                    .gap_1()
                    .child(
                        Button::new(("cancel-rule", index))
                            .ghost()
                            .compact()
                            .label("Cancel")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.show_rule_form(&cancel, false, cx)
                            })),
                    )
                    .child(
                        Button::new(("save-rule", index))
                            .ghost()
                            .compact()
                            .label("Add rule")
                            .disabled(!ready)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.add_rule(&add, window, cx)
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_new_identity(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let labelled = |text: &'static str, input: &Entity<InputState>| {
            div()
                .v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(section_label(muted).child(text))
                .child(Input::new(input))
        };
        let ready = !self.new_label.read(cx).value().trim().is_empty()
            && [
                &self.new_identity_name,
                &self.new_identity_email,
                &self.new_identity_key,
                &self.new_identity_ssh,
            ]
            .iter()
            .any(|input| !input.read(cx).value().trim().is_empty());
        self.render_group(
            "Add an identity",
            vec![
                div()
                    .v_flex()
                    .px_4()
                    .py_3()
                    .gap_3()
                    .child(labelled("What to call it", &self.new_label))
                    .child(
                        div()
                            .h_flex()
                            .gap_3()
                            .child(labelled("Name", &self.new_identity_name))
                            .child(labelled("Email", &self.new_identity_email))
                            .child(labelled("Signing key", &self.new_identity_key)),
                    )
                    .child(self.render_ssh_key(
                        KeyTarget::New,
                        usize::MAX / 200,
                        &self.new_identity_ssh,
                        cx,
                    ))
                    .child(
                        div().h_flex().justify_end().child(
                            Button::new("add-identity")
                                .ghost()
                                .compact()
                                .label("Add identity")
                                .disabled(!ready)
                                .on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.add_identity(window, cx)
                                    }),
                                ),
                        ),
                    )
                    .into_any_element(),
            ],
            cx,
        )
    }
}
