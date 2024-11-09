use crate::{
    config::{NotifyZulipConfig, NotifyZulipLabelConfig, NotifyZulipNameConfig},
    github::{Issue, IssuesAction, IssuesEvent, Label},
    handlers::Context,
};
use tracing as log;

pub(super) struct NotifyZulipInput {
    notification_type: NotificationType,
    /// Label that triggered this notification.
    ///
    /// For example, if an `I-prioritize` issue is closed,
    /// this field will be `I-prioritize`.
    label: Label,
    is_default_valid: bool,
    names: Vec<String>,
}

pub(super) enum NotificationType {
    Labeled,
    Unlabeled,
    Closed,
    Reopened,
}

pub(super) async fn parse_input(
    _ctx: &Context,
    event: &IssuesEvent,
    config: Option<&NotifyZulipConfig>,
) -> Result<Option<Vec<NotifyZulipInput>>, String> {
    let config = match config {
        Some(config) => config,
        None => return Ok(None),
    };

    match &event.action {
        IssuesAction::Labeled { label } | IssuesAction::Unlabeled { label } => {
            let applied_label = label.clone();
            Ok(config
                .labels
                .get(&applied_label.name)
                .and_then(|label_config| {
                    parse_label_change_input(event, applied_label, label_config)
                })
                .map(|input| vec![input]))
        }
        IssuesAction::Closed | IssuesAction::Reopened => {
            Ok(Some(parse_close_reopen_input(event, config)))
        }
        _ => Ok(None),
    }
}

fn parse_label_change_input(
    event: &IssuesEvent,
    label: Label,
    config: &NotifyZulipNameConfig,
) -> Option<NotifyZulipInput> {
    let mut is_default_valid = false;
    let mut names: Vec<String> = vec![];

    match &config.default {
        Some(label_config) => {
            if has_all_required_labels(&event.issue, &label_config) {
                match event.action {
                    IssuesAction::Labeled { .. } if !label_config.messages_on_add.is_empty() => {
                        is_default_valid = true;
                    }
                    IssuesAction::Unlabeled { .. }
                        if !label_config.messages_on_remove.is_empty() =>
                    {
                        is_default_valid = true;
                    }
                    _ => (),
                }
            }
        }
        None => (),
    }

    match &config.others {
        Some(other_configs) => {
            for (name, label_config) in other_configs {
                if has_all_required_labels(&event.issue, &label_config) {
                    match event.action {
                        IssuesAction::Labeled { .. }
                            if !label_config.messages_on_add.is_empty() =>
                        {
                            names.push(name.to_string());
                        }
                        IssuesAction::Unlabeled { .. }
                            if !label_config.messages_on_remove.is_empty() =>
                        {
                            names.push(name.to_string());
                        }
                        _ => (),
                    }
                }
            }
        }
        None => (),
    }

    if !is_default_valid && names.is_empty() {
        // It seems that there is no match between this event and any notify-zulip config, ignore this event
        return None;
    }

    match event.action {
        IssuesAction::Labeled { .. } => Some(NotifyZulipInput {
            notification_type: NotificationType::Labeled,
            label,
            is_default_valid,
            names,
        }),
        IssuesAction::Unlabeled { .. } => Some(NotifyZulipInput {
            notification_type: NotificationType::Unlabeled,
            label,
            is_default_valid,
            names,
        }),
        _ => None,
    }
}

fn parse_close_reopen_input(
    event: &IssuesEvent,
    global_config: &NotifyZulipConfig,
) -> Vec<NotifyZulipInput> {
    event
        .issue
        .labels
        .iter()
        .cloned()
        .filter_map(|label| {
            global_config
                .labels
                .get(&label.name)
                .map(|config| (label, config))
        })
        .flat_map(|(label, config)| {
            let mut is_default_valid = false;
            let mut names: Vec<String> = vec![];

            match &config.default {
                Some(label_config) => {
                    if has_all_required_labels(&event.issue, &label_config) {
                        match event.action {
                            IssuesAction::Closed if !label_config.messages_on_close.is_empty() => {
                                is_default_valid = true;
                            }
                            IssuesAction::Reopened
                                if !label_config.messages_on_reopen.is_empty() =>
                            {
                                is_default_valid = true;
                            }
                            _ => (),
                        }
                    }
                }
                None => (),
            }

            match &config.others {
                Some(other_configs) => {
                    for (name, label_config) in other_configs {
                        if has_all_required_labels(&event.issue, &label_config) {
                            match event.action {
                                IssuesAction::Closed
                                    if !label_config.messages_on_close.is_empty() =>
                                {
                                    names.push(name.to_string());
                                }
                                IssuesAction::Reopened
                                    if !label_config.messages_on_reopen.is_empty() =>
                                {
                                    names.push(name.to_string());
                                }
                                _ => (),
                            }
                        }
                    }
                }
                None => (),
            }

            if !is_default_valid && names.is_empty() {
                // It seems that there is no match between this event and any notify-zulip config, ignore this event
                return None;
            }

            match event.action {
                IssuesAction::Closed => Some(NotifyZulipInput {
                    notification_type: NotificationType::Closed,
                    label,
                    is_default_valid,
                    names,
                }),
                IssuesAction::Reopened => Some(NotifyZulipInput {
                    notification_type: NotificationType::Reopened,
                    label,
                    is_default_valid,
                    names,
                }),
                _ => None,
            }
        })
        .collect()
}

fn has_all_required_labels(issue: &Issue, config: &NotifyZulipLabelConfig) -> bool {
    for req_label in &config.required_labels {
        let pattern = match glob::Pattern::new(req_label) {
            Ok(pattern) => pattern,
            Err(err) => {
                log::error!("Invalid glob pattern: {}", err);
                continue;
            }
        };
        if !issue.labels().iter().any(|l| pattern.matches(&l.name)) {
            return false;
        }
    }

    true
}

pub(super) async fn handle_input<'a>(
    ctx: &Context,
    config: &NotifyZulipConfig,
    event: &IssuesEvent,
    inputs: Vec<NotifyZulipInput>,
) -> anyhow::Result<()> {
    for input in inputs {
        let name_config = &config.labels[&input.label.name];

        // Get valid label configs
        let mut label_configs: Vec<&NotifyZulipLabelConfig> = vec![];
        if input.is_default_valid {
            label_configs.push(name_config.default.as_ref().unwrap());
        }
        for name in input.names {
            label_configs.push(&name_config.others.as_ref().unwrap()[&name]);
        }

        for label_config in label_configs {
            let config = label_config;

            let topic = &config.topic;
            let topic = topic.replace("{number}", &event.issue.number.to_string());
            let mut topic = topic.replace("{title}", &event.issue.title);
            // Truncate to 60 chars (a Zulip limitation)
            let mut chars = topic.char_indices().skip(59);
            if let (Some((len, _)), Some(_)) = (chars.next(), chars.next()) {
                topic.truncate(len);
                topic.push('…');
            }

            let msgs = match input.notification_type {
                NotificationType::Labeled => &config.messages_on_add,
                NotificationType::Unlabeled => &config.messages_on_remove,
                NotificationType::Closed => &config.messages_on_close,
                NotificationType::Reopened => &config.messages_on_reopen,
            };

            let recipient = crate::zulip::Recipient::Stream {
                id: config.zulip_stream,
                topic: &topic,
            };

            for msg in msgs {
                let msg = msg.replace("{number}", &event.issue.number.to_string());
                let msg = msg.replace("{title}", &event.issue.title);
                let msg = replace_team_to_be_nominated(&event.issue.labels, msg);

                crate::zulip::MessageApiRequest {
                    recipient,
                    content: &msg,
                }
                .send(&ctx.github.raw())
                .await?;
            }
        }
    }

    Ok(())
}

fn replace_team_to_be_nominated(labels: &[Label], msg: String) -> String {
    let teams = labels
        .iter()
        .map(|label| &label.name)
        .filter_map(|label| label.strip_prefix("T-"))
        .collect::<Vec<&str>>();

    // - If a single team label is found, replace the placeholder with that one
    // - If multiple team labels are found and one of them is "compiler", pick that one
    //   (currently the only team handling these Zulip notification)
    // - else, do nothing
    if let [team] = &*teams {
        msg.replace("{team}", team)
    } else if teams.contains(&"compiler") {
        msg.replace("{team}", "compiler")
    } else {
        msg
    }
}

#[test]
fn test_notification() {
    let mut msg = replace_team_to_be_nominated(&[], "Needs `I-{team}-nominated`?".to_string());
    assert!(msg.contains("Needs `I-{team}-nominated`?"), "{}", msg);

    msg = replace_team_to_be_nominated(
        &[Label {
            name: "T-cooks".to_string(),
        }],
        "Needs `I-{team}-nominated`?".to_string(),
    );
    assert!(msg.contains("I-cooks-nominated"), "{}", msg);

    msg = replace_team_to_be_nominated(
        &[
            Label {
                name: "T-compiler".to_string(),
            },
            Label {
                name: "T-libs".to_string(),
            },
            Label {
                name: "T-cooks".to_string(),
            },
        ],
        "Needs `I-{team}-nominated`?".to_string(),
    );
    assert!(msg.contains("I-compiler-nominated"), "{}", msg);

    msg = replace_team_to_be_nominated(
        &[
            Label {
                name: "T-libs".to_string(),
            },
            Label {
                name: "T-cooks".to_string(),
            },
        ],
        "Needs `I-{team}-nominated`?".to_string(),
    );
    assert!(msg.contains("Needs `I-{team}-nominated`?"), "{}", msg);
}
