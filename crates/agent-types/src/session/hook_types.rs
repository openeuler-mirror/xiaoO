#[derive(Clone, Debug)]
pub struct SessionCreatedHookInput {
    pub session_id: String,
    pub sender_id: String,
}

#[derive(Clone, Debug)]
pub struct SessionClosedHookInput {
    pub session_id: String,
    pub sender_id: String,
}

#[derive(Clone, Debug)]
pub enum SessionHookResult {
    Acknowledged,
}
