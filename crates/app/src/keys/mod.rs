//! The keyboard. `map` holds the bindings as data (each action's default keys, the user's
//! changes from settings.json, the fixed keys, conflicts); `shortcuts` runs the shortcut a key
//! press is bound to and says where a list's cursor moves; `edit` is Settings > Keys, the
//! editor.

pub(crate) mod edit;
pub(crate) mod map;
pub(crate) mod shortcuts;
