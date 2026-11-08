//! The Git config page: one config at a time, Everywhere or Only this repository, as
//! cards of settings with their sources, and the aliases.

use super::*;

impl Settings {
    pub(super) fn commit_text(
        &mut self,
        key: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(field) = self.fields.iter().find(|field| field.key == key) else {
            return;
        };
        let typed = field.input.read(cx).value().trim().to_owned();
        let current = self.values.as_ref().and_then(|values| values.get(key));
        if current.unwrap_or("") != typed {
            self.write(key, &typed, window, cx);
        }
    }

    pub(super) fn commit_alias(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.aliases.iter().find(|row| row.name == name) else {
            return;
        };
        let typed = row.input.read(cx).value().trim().to_owned();
        let current = self.values.as_ref().and_then(|values| {
            values
                .aliases()
                .into_iter()
                .find(|alias| alias.name == name)
                .map(|alias| alias.command)
        });
        if current.as_deref() == Some(typed.as_str()) {
            return;
        }
        let name = name.to_owned();
        let saved = format!("Saved git {name} as {typed}.");
        self.change(
            saved,
            move |scope, _| config::set_alias(scope, &name, &typed),
            None,
            window,
            cx,
        );
    }

    pub(super) fn add_alias(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.new_name.read(cx).value().trim().to_owned();
        let command = self.new_command.read(cx).value().trim().to_owned();
        if let Some(problem) = config::alias_problem(&name) {
            self.feedback = Feedback::Failed(problem.to_owned());
            cx.notify();
            return;
        }
        let saved = format!("Added git {name}.");
        self.change(
            saved,
            move |scope, _| config::set_alias(scope, &name, &command),
            Some(|this, window, cx| {
                for input in [&this.new_name, &this.new_command] {
                    input.update(cx, |input, cx| input.set_value("", window, cx));
                }
            }),
            window,
            cx,
        );
    }

    fn remove_alias(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let name = name.to_owned();
        let saved = format!("Removed git {name}.");
        self.change(
            saved,
            move |scope, _| config::remove_alias(scope, &name),
            None,
            window,
            cx,
        );
    }

    pub(super) fn render_git_config(&self, cx: &mut Context<Self>) -> AnyElement {
        let (muted, danger) = (cx.theme().muted_foreground, cx.theme().danger);
        let sentence = match self.target {
            Target::Repository if self.repo.is_some() => {
                "These apply only to this repository, and win over your global config."
            }
            _ => "These apply to every repository on this computer.",
        };
        let mut page = div()
            .v_flex()
            .w_full()
            .max_w(PAGE_WIDTH)
            .gap_6()
            .child(self.page_head("Git config", sentence, cx))
            .child(self.render_target(cx));

        if let Some(Err(message)) = &self.git {
            return page
                .child(self.render_no_git(message, cx))
                .into_any_element();
        }
        let Some(values) = self.values.clone() else {
            let (text, color) = match &self.load_error {
                Some(error) => (error.clone(), danger),
                None => ("Reading your git config…".to_owned(), muted),
            };
            return page
                .child(div().text_size(TEXT_BODY).text_color(color).child(text))
                .into_any_element();
        };

        let version = self.git.as_ref().and_then(|git| git.as_ref().ok()).copied();
        for (index, section) in config::sections().iter().enumerate() {
            let rows: Vec<AnyElement> = section
                .settings
                .iter()
                .enumerate()
                .map(|(row, setting)| {
                    self.render_setting(index * 100 + row, setting, &values, version, cx)
                })
                .collect();
            page = page.child(self.render_group(section.title, rows, cx));
        }
        page.child(self.render_aliases(cx)).into_any_element()
    }

    /// Everywhere, or only the open repository.
    fn render_target(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let Some(repo) = self.repo.as_deref() else {
            return div()
                .text_size(TEXT_SMALL)
                .text_color(muted)
                .child("Open a repository to change its settings on their own.")
                .into_any_element();
        };
        let name = repository_name(repo);
        div()
            .h_flex()
            .gap_1()
            .child(
                chip(
                    "target-everywhere",
                    "Everywhere",
                    self.target == Target::Everywhere,
                    cx,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.set_target(Target::Everywhere, window, cx)
                })),
            )
            .child(
                chip(
                    "target-repository",
                    &format!("Only {name}"),
                    self.target == Target::Repository,
                    cx,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.set_target(Target::Repository, window, cx)
                })),
            )
            .into_any_element()
    }

    fn render_setting(
        &self,
        id: usize,
        setting: &Setting,
        values: &Values,
        version: Option<Version>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let setting = *setting;
        let scope = self.scope();
        let muted = cx.theme().muted_foreground;
        let inherited = self.sources.inherited(setting.key, values, &scope);
        let too_old = version.and_then(|git| setting.unavailable(git));
        let hint = too_old.clone().unwrap_or_else(|| setting.hint.to_owned());
        let words = div()
            .v_flex()
            .flex_1()
            .min_w_0()
            .gap_0p5()
            .when(too_old.is_some(), |words| words.opacity(0.6))
            .child(
                div()
                    .text_size(TEXT_BODY)
                    .line_height(px(20.))
                    .child(setting.label),
            )
            .child(
                div()
                    .text_size(TEXT_SMALL)
                    .line_height(px(17.))
                    .text_color(muted)
                    .child(hint),
            )
            .children(
                inherited
                    .as_ref()
                    .filter(|_| too_old.is_none())
                    .map(|(value, note)| {
                        let shown = if value.is_empty() {
                            note.clone()
                        } else {
                            format!("{} {value}", note.trim_end_matches('.').to_owned() + ":")
                        };
                        div()
                            .text_size(TEXT_SMALL)
                            .line_height(px(17.))
                            .text_color(muted)
                            .child(shown)
                    }),
            );
        let row = div().px_4().py_3().gap_3();
        if too_old.is_some() {
            return row.h_flex().child(words).into_any_element();
        }

        match setting.kind {
            Kind::Text { .. } => {
                let input = self
                    .fields
                    .iter()
                    .find(|field| field.key == setting.key)
                    .map(|field| field.input.clone());
                row.h_flex()
                    .gap_4()
                    .child(words)
                    .children(
                        input.map(|input| {
                            div().w(INPUT_WIDTH).flex_none().child(Input::new(&input))
                        }),
                    )
                    .into_any_element()
            }
            Kind::Toggle { .. } => {
                let on = self.sources.is_on(&setting);
                let on = if values.is_set(setting.key) {
                    values.is_on(&setting)
                } else {
                    on
                };
                let key = setting.key;
                let reset = values.is_set(key).then(|| {
                    Button::new(("reset", id))
                        .ghost()
                        .compact()
                        .label(scope.reset_label())
                        .tooltip(scope.reset_hint())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.write(key, "", window, cx);
                        }))
                });
                row.h_flex()
                    .gap_4()
                    .child(words)
                    .children(reset)
                    .child(
                        checkbox(("toggle", id), on, false, cx).on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.write(key, if on { "false" } else { "true" }, window, cx);
                            },
                        )),
                    )
                    .into_any_element()
            }
            Kind::Choice { options } => {
                let key = setting.key;
                let current = values.get(key);
                let unknown = current.filter(|value| !options.iter().any(|(v, _)| v == value));
                let git = version.unwrap_or(Version(u32::MAX, 0));
                let mut chips = vec![
                    chip(
                        ("choice", id * 10),
                        scope.reset_label_for_choice(),
                        current.is_none(),
                        cx,
                    )
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.write(key, "", window, cx)),
                    ),
                ];
                for (position, (value, label)) in options.iter().enumerate() {
                    if !config::option_available(key, value, git) {
                        continue;
                    }
                    let on = current == Some(*value);
                    let value = *value;
                    chips.push(
                        chip(("choice", id * 10 + position + 1), label, on, cx).on_click(
                            cx.listener(move |this, _, window, cx| {
                                this.write(key, value, window, cx)
                            }),
                        ),
                    );
                }
                // Something set by hand, which no chip stands for, stays visible.
                if let Some(value) = unknown {
                    chips.push(chip(("choice", id * 10 + 9), value, true, cx));
                }
                row.v_flex()
                    .child(words)
                    .child(div().flex().flex_wrap().gap_1().children(chips))
                    .into_any_element()
            }
        }
    }

    fn render_aliases(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let mut rows: Vec<AnyElement> = Vec::new();
        let listed = self
            .values
            .as_ref()
            .map_or(0, |values| values.aliases().len());
        if listed == 0 {
            rows.push(
                div()
                    .px_4()
                    .py_3()
                    .text_size(TEXT_SMALL)
                    .line_height(px(17.))
                    .text_color(muted)
                    .child(
                        "No aliases yet. An alias named co that runs checkout lets you type git co.",
                    )
                    .into_any_element(),
            );
        }
        for (index, row) in self.aliases.iter().enumerate() {
            let name = row.name.clone();
            rows.push(
                div()
                    .h_flex()
                    .px_4()
                    .py_2()
                    .gap_3()
                    .child(
                        div()
                            .w(NAME_WIDTH)
                            .flex_none()
                            .truncate()
                            .text_size(TEXT_BODY)
                            .child(format!("git {}", row.name)),
                    )
                    .child(div().flex_1().min_w_0().child(Input::new(&row.input)))
                    .child(
                        Button::new(("remove-alias", index))
                            .ghost()
                            .compact()
                            .label("Remove")
                            .tooltip(format!("Delete the alias git {}", row.name))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.remove_alias(&name, window, cx)
                            })),
                    )
                    .into_any_element(),
            );
        }

        let ready = config::alias_problem(self.new_name.read(cx).value().trim()).is_none()
            && !self.new_command.read(cx).value().trim().is_empty();
        rows.push(
            div()
                .h_flex()
                .px_4()
                .py_2()
                .gap_3()
                .child(
                    div()
                        .w(NAME_WIDTH)
                        .flex_none()
                        .child(Input::new(&self.new_name)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Input::new(&self.new_command)),
                )
                .child(
                    Button::new("add-alias")
                        .ghost()
                        .compact()
                        .label("Add alias")
                        .disabled(!ready)
                        .on_click(cx.listener(|this, _, window, cx| this.add_alias(window, cx))),
                )
                .into_any_element(),
        );
        self.render_group("Aliases", rows, cx)
    }
}
