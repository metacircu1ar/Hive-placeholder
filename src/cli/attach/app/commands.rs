//! Commands for the App view.
use super::{
    ARGUMENTS, App, Command, Exit, Focus, HIDDEN_QUESTION, Handled, Kind, Picker, Value, options,
};

use super::requests::{self, Request};

impl App {
    /// Send the input: a view command, or a message whose mode follows the
    /// agent. Any other `/text` is forwarded to the agent as a message, and
    /// `//text` sends `/text` as it is.
    pub(super) fn submit(&mut self, mode: Option<&str>) {
        let text = self.input.trim().to_owned();
        if text.is_empty() {
            return;
        }
        let mut label = None;
        let body = match text.strip_prefix('/') {
            Some(literal) if literal.starts_with('/') => literal.to_owned(),
            Some(command) => match self.command(command) {
                Handled::Done => {
                    self.clear_input();
                    return;
                }
                Handled::Kept => return,
                Handled::Forward => {
                    let name = command.split_whitespace().next().unwrap_or_default();
                    label = Some(format!("forward /{name} to the agent"));
                    text
                }
            },
            None => text,
        };
        let header = self.model.as_ref().map(|model| &model.header);
        let mode = mode.unwrap_or(match header {
            Some(header) if header.running() && header.steering => "steer",
            _ => "next-turn",
        });
        let label = label.unwrap_or_else(|| {
            String::from(match mode {
                "steer" => "steer",
                "redirect" => "redirect",
                _ => "send",
            })
        });
        self.act(
            &label,
            requests::value(Request::Send {
                target: &self.target,
                body: &body,
                mode,
            }),
        );
        self.clear_input();
    }

    pub(super) fn clear_input(&mut self) {
        self.input.clear();
        self.cursor = 0;
    }

    /// Run a view command. A known command with the wrong arguments keeps the
    /// input for correction; an unknown one is for the agent.
    pub(super) fn command(&mut self, line: &str) -> Handled {
        let (name, rest) = line.split_once(' ').unwrap_or((line, ""));
        let rest = rest.trim();
        if name.is_empty() {
            return self.usage("/help lists the view commands");
        }
        if let Some(handled) = self.local_command(name, rest) {
            return handled;
        }
        let target = &self.target;
        let request = match (name, rest.is_empty()) {
            ("name", false) => requests::value(Request::SetName { target, name: rest }),
            ("model" | "effort", false) => requests::option(target, name, rest),

            ("options", false) => match options(rest) {
                Some(options) => requests::options(target, options),
                None => return self.usage("/options KEY=VALUE ..."),
            },
            ("auth", false) => requests::value(Request::SetAuth { target, auth: rest }),
            ("stop" | "resume" | "terminate" | "interrupt", true) => {
                requests::control(name, target)
            }
            ("complete", false) => requests::value(Request::Complete { target, body: rest }),
            ("branch", false) => requests::value(Request::Spawn {
                from_agent: Some(target),
                role: None,
                body: rest,
                cwd: None,
            }),
            ("spawn", false) => {
                let Some((role, body)) = rest.split_once(' ') else {
                    return self.usage("/spawn ROLE TEXT");
                };
                let Ok(cwd) = std::env::current_dir() else {
                    self.status =
                        String::from("spawn failed: the current directory is unavailable");
                    return Handled::Kept;
                };
                requests::value(Request::Spawn {
                    from_agent: None,
                    role: Some(role),
                    body: body.trim(),
                    cwd: Some(&cwd),
                })
            }

            (name, _) => {
                return match ARGUMENTS.iter().find(|(command, _)| *command == name) {
                    Some((_, arguments)) => self.usage(arguments),
                    None => Handled::Forward,
                };
            }
        };
        self.act(name, request);
        Handled::Done
    }

    /// Read the agent's login usage; the reply becomes a transcript notice.
    pub(super) fn read_usage(&mut self) {
        let auth = self
            .model
            .as_ref()
            .map(|model| model.header.auth.clone())
            .unwrap_or_default();
        if auth.is_empty() {
            self.status = String::from("usage: the agent's login is not known yet");
            return;
        }
        self.status = format!("reading {auth} usage…");
        self.commands.push(Command::Usage { auth });
    }

    pub(super) fn usage_read(&mut self, auth: &str, result: Result<Value, String>) {
        match result {
            Ok(reports) => {
                let generation = self
                    .model
                    .as_ref()
                    .and_then(|model| model.header.auth_generation.clone());
                let now = crate::state::now();
                self.note(super::super::usage::summary(
                    auth,
                    generation.as_deref(),
                    &reports,
                    now,
                ));
                self.status = self.hint();
            }
            Err(problem) => self.status = format!("usage failed: {problem}"),
        }
    }

    /// A local transcript notice, or the status before the first snapshot.
    pub(super) fn note(&mut self, text: String) {
        match &mut self.model {
            Some(model) => model.note(text),
            None => self.status = text,
        }
    }

    /// Open the picker over the names listed for the agent's provider; with
    /// none listed, show the command's arguments instead.
    pub(super) fn open_picker(&mut self, kind: Kind) -> Handled {
        let picker = self.model.as_ref().and_then(|model| {
            let header = &model.header;
            let current = match kind {
                Kind::Model => &header.model,
                Kind::Effort => &header.effort,
            };
            let provider = header.provider.as_deref()?;
            Picker::new(
                kind,
                provider,
                self.mode.lists.names(provider, kind),
                current,
            )
        });
        let Some(picker) = picker else {
            return self.usage(match kind {
                Kind::Model => "/model MODEL",
                Kind::Effort => "/effort EFFORT",
            });
        };
        self.picker = Some(picker);
        self.focus = Focus::Card;
        self.note_hidden_question();
        Handled::Done
    }

    /// While the picker covers a waiting question, the status says so; the
    /// note goes once either is gone.
    pub(super) fn note_hidden_question(&mut self) {
        if self.picker.is_some() && self.card.is_some() {
            self.status = String::from(HIDDEN_QUESTION);
        } else if self.status == HIDDEN_QUESTION {
            self.status = self.hint();
        }
    }

    /// Set the picker's choice, as `/model NAME` or `/effort NAME` would.
    pub(super) fn pick(&mut self) {
        let Some(picker) = self.picker.take() else {
            return;
        };
        let option = picker.kind.option();
        let request = requests::option(&self.target, option, picker.chosen());
        self.close_picker();
        self.act(option, request);
    }

    /// Close the picker; a waiting question shows again.
    pub(super) fn close_picker(&mut self) {
        self.drop_picker();
        self.focus = Focus::Input;
    }

    /// Remove the picker, and the status note about the question it covered.
    pub(super) fn drop_picker(&mut self) {
        self.picker = None;
        if self.status == HIDDEN_QUESTION {
            self.status = self.hint();
        }
    }

    /// A question or a picker is shown in the card's place.
    pub(super) fn has_card(&self) -> bool {
        self.card.is_some() || self.picker.is_some()
    }

    pub(super) fn usage(&mut self, text: &str) -> Handled {
        self.status = format!("usage: {text}");
        Handled::Kept
    }

    pub(super) fn respond(&mut self, action: &str) {
        let Some(card) = &self.card else {
            self.status = String::from("no question is waiting");
            return;
        };
        if self.mode.read_only {
            return;
        }
        let mut content = None;
        if action == "accept" {
            match card.content() {
                Ok(Some(value)) => content = Some(value),
                Ok(None) => {}
                Err(problem) => {
                    self.status = problem;
                    self.focus = Focus::Card;
                    return;
                }
            }
        }
        let request = requests::value(Request::Respond {
            approval_id: &card.id,
            action,
            content,
        });
        self.act(action, request);
    }

    pub(super) fn act(&mut self, label: &str, request: Value) {
        if self.mode.read_only {
            return;
        }
        self.actions += 1;
        self.in_flight.push(self.actions);
        self.status = format!("{label}…");
        self.commands.push(Command::Write {
            action: self.actions,
            id: uuid::Uuid::new_v4().to_string(),
            label: label.to_owned(),
            request,
        });
    }
}

impl App {
    fn local_command(&mut self, name: &str, rest: &str) -> Option<Handled> {
        match (name, rest.is_empty()) {
            ("quit", true) => {
                self.exit = Some(Exit::Quit);
                Some(Handled::Done)
            }
            ("attach", false) => {
                self.exit = Some(Exit::Attach(rest.to_owned()));
                Some(Handled::Done)
            }
            ("model", true) => Some(self.open_picker(Kind::Model)),
            ("effort", true) => Some(self.open_picker(Kind::Effort)),
            ("accept" | "decline" | "cancel", true) => {
                self.respond(name);
                Some(Handled::Done)
            }
            ("usage", true) => {
                self.read_usage();
                Some(Handled::Done)
            }
            ("id", true) => {
                self.copy_id();
                Some(Handled::Done)
            }
            ("help", true) => {
                let commands: Vec<String> = ARGUMENTS
                    .iter()
                    .map(|(name, _)| format!("/{name}"))
                    .collect();
                self.note(format!(
                    "view commands: {}; any other /command is sent to the agent, and //text \
                        sends /text",
                    commands.join(" ")
                ));
                Some(Handled::Done)
            }
            _ => None,
        }
    }
}
