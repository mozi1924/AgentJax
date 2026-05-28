/// Upper bound on the number of request input items we keep after rebuilding
/// a conversation context snapshot.
///
/// Keeping the budget local to the context module makes it easy to add future
/// policy variants without leaking the limit into persistence code.
pub(super) const MAX_CONTEXT_ITEMS_PER_REQUEST: usize = 200;
