use crate::chat::MessageRole;

#[derive(Debug, Clone)]
pub struct TurnSummary {
    pub index: usize,
    /// 0-based position among User messages; used for positional matching.
    pub turn_index: usize,
    pub full_content: String,
    pub msg_range: (usize, usize),
}

/// State machine for the delete dialog: selecting a turn, then confirming.
#[derive(Debug, Clone)]
pub enum DeleteDialog {
    Selecting {
        entries: Vec<TurnSummary>,
        selected: usize,
    },
    Confirming {
        turn: TurnSummary,
        subsequent_count: usize,
    },
}

impl DeleteDialog {
    /// Returns None if there are no user messages.
    pub fn new(messages: &[crate::chat::Message]) -> Option<Self> {
        let entries = group_into_turns(messages);
        if entries.is_empty() {
            return None;
        }
        Some(Self::Selecting {
            entries,
            selected: 0,
        })
    }

    pub fn move_up(&mut self) {
        if let DeleteDialog::Selecting { selected, .. } = self {
            *selected = selected.saturating_sub(1);
        }
    }

    pub fn move_down(&mut self) {
        if let DeleteDialog::Selecting { entries, selected } = self {
            if !entries.is_empty() {
                *selected = (*selected + 1).min(entries.len() - 1);
            }
        }
    }

    pub fn selected_turn(&self) -> Option<&TurnSummary> {
        match self {
            DeleteDialog::Selecting { entries, selected } => entries.get(*selected),
            DeleteDialog::Confirming { turn, .. } => Some(turn),
        }
    }

    /// Transition from Selecting to Confirming. Returns false if not in Selecting.
    pub fn advance_to_confirm(&mut self) -> bool {
        let (turn, subsequent_count) = match self {
            DeleteDialog::Selecting { entries, selected } => {
                if *selected >= entries.len() {
                    return false;
                }
                let turn = entries[*selected].clone();
                let subsequent = entries.len() - *selected - 1;
                (turn, subsequent)
            }
            _ => return false,
        };
        *self = DeleteDialog::Confirming {
            turn,
            subsequent_count,
        };
        true
    }

    pub fn is_selecting(&self) -> bool {
        matches!(self, DeleteDialog::Selecting { .. })
    }
}

/// Groups messages into turns. A turn starts at each User message and extends
/// until the next User message (exclusive). System/Error messages are skipped.
fn group_into_turns(messages: &[crate::chat::Message]) -> Vec<TurnSummary> {
    let mut turns: Vec<(usize, usize, usize)> = Vec::new();
    let mut turn_start: Option<usize> = None;

    for (i, msg) in messages.iter().enumerate() {
        if msg.role == MessageRole::User {
            if let Some(start) = turn_start.take() {
                turns.push((turns.len(), start, i));
            }
            turn_start = Some(i);
        }
    }
    if let Some(start) = turn_start.take() {
        turns.push((turns.len(), start, messages.len()));
    }

    turns
        .into_iter()
        .map(|(idx, start, end)| {
            let full_content = messages[start].content.clone();
            TurnSummary {
                index: idx + 1,
                turn_index: idx,
                full_content,
                msg_range: (start, end),
            }
        })
        .collect()
}

/// Removes the `turn_index`-th User message and all messages until the next
/// User message (exclusive).
pub fn remove_turn_from_session_messages(
    session_messages: &mut Vec<llm_client::ChatMessage>,
    turn_index: usize,
) {
    use agent_types::llm::message::MessageRole;

    let mut user_count = 0;
    let mut start = None;
    for (i, msg) in session_messages.iter().enumerate() {
        if msg.role == MessageRole::User {
            if user_count == turn_index {
                start = Some(i);
                break;
            }
            user_count += 1;
        }
    }

    let Some(start) = start else {
        return;
    };

    let end = session_messages[start + 1..]
        .iter()
        .position(|msg| msg.role == MessageRole::User)
        .map(|pos| start + 1 + pos)
        .unwrap_or(session_messages.len());

    session_messages.drain(start..end);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::Message;

    #[test]
    fn empty_messages_returns_none() {
        assert!(DeleteDialog::new(&[]).is_none());
    }

    #[test]
    fn system_only_messages_returns_none() {
        let msgs = vec![Message::system("hello")];
        assert!(DeleteDialog::new(&msgs).is_none());
    }

    #[test]
    fn single_user_message_creates_one_turn() {
        let msgs = vec![
            Message::system("welcome"),
            Message::user("help me"),
            Message::assistant_streaming(),
        ];
        let dialog = DeleteDialog::new(&msgs).unwrap();
        let entries = match &dialog {
            DeleteDialog::Selecting { entries, .. } => entries,
            _ => panic!("expected Selecting"),
        };
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].full_content, "help me");
        // msg_range should be [1, 3) — skip the system message
        assert_eq!(entries[0].msg_range, (1, 3));
    }

    #[test]
    fn multiple_turns_group_correctly() {
        let msgs = vec![
            Message::user("turn 1 question"),
            Message::assistant_streaming(),
            Message::user("turn 2 question"),
            Message::assistant_streaming(),
        ];
        let dialog = DeleteDialog::new(&msgs).unwrap();
        let entries = match &dialog {
            DeleteDialog::Selecting { entries, .. } => entries,
            _ => panic!("expected Selecting"),
        };
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].msg_range, (0, 2));
        assert_eq!(entries[1].msg_range, (2, 4));
    }

    #[test]
    fn advance_to_confirm_transitions_state() {
        let msgs = vec![
            Message::user("q1"),
            Message::assistant_streaming(),
            Message::user("q2"),
        ];
        let mut dialog = DeleteDialog::new(&msgs).unwrap();
        assert!(dialog.is_selecting());

        dialog.advance_to_confirm();
        assert!(!dialog.is_selecting());
        let (subsequent, content) = match &dialog {
            DeleteDialog::Confirming { turn, subsequent_count } => (*subsequent_count, turn.full_content.clone()),
            _ => panic!("expected Confirming"),
        };
        assert_eq!(subsequent, 1); // q2 is after q1
        assert_eq!(content, "q1");
    }

    #[test]
    fn long_content_is_preserved_in_full() {
        let long_msg = "a".repeat(100);
        let msgs = vec![Message::user(long_msg.clone())];
        let dialog = DeleteDialog::new(&msgs).unwrap();
        let entries = match &dialog {
            DeleteDialog::Selecting { entries, .. } => entries,
            _ => panic!("expected Selecting"),
        };
        assert_eq!(entries[0].full_content, long_msg);
    }
}