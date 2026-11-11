use serde::Serialize;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct HistoryCommittedEvent<'a> {
    pub(super) history_revision: u64,
    pub(super) record_id: Option<&'a str>,
    pub(super) action: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) affected_count: Option<u64>,
}
