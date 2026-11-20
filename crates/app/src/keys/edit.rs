//! Settings > Keys, the shortcut editor. A row per remappable action (`keys::map::Action`)
//! with a button per key slot: activating one waits for the next key press and makes it
//! the action's (Esc cancels, Delete leaves the action without a key). A key another
//! action has is not taken silently: the editor names that action and asks whether to
//! reassign it (Enter, or the button) or cancel (Esc). Fixed keys are refused with the
//! reason. Every change is saved at once (only what differs from the defaults), and the
//! labels and tooltips that name a key (`Keys` in stores.slint) follow it.

use std::cell::RefCell;
use std::rc::Rc;

use slint::platform::Key;
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::keys::map::{Action, Assign, Binding, fixed_help};
use crate::state::State;
use crate::{KeyHelp, KeyRow, Keys};

thread_local! {
    /// A captured key that is another action's, waiting for Reassign or Cancel.
    static PENDING: RefCell<Option<Binding>> = const { RefCell::new(None) };
}

fn key_name(b: &Binding) -> String {
    b.display()
}

/// Fill Settings > Keys and every label that names a key, and tell the window which pin
/// keys to take before focus. Run at start and after every change.
pub fn refresh(st: &Rc<State>) {
    crate::keys::shortcuts::sync_window_keys(&st.keymap.borrow());
    let Some(app) = st.app.upgrade() else { return };
    let km = st.keymap.borrow();
    let g = app.global::<Keys>();
    let shown = |b: &Option<Binding>| b.as_ref().map(key_name).unwrap_or_default();
    let spoken = |b: &Option<Binding>| b.as_ref().map(Binding::config).unwrap_or_default();
    let rows: Vec<KeyRow> = Action::ALL
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let k = km.bindings(*a);
            let none = None;
            let k1 = k.get(1).unwrap_or(&none);
            KeyRow {
                what: a.describe().into(),
                key0: shown(&k[0]).into(),
                key1: shown(k1).into(),
                spoken0: spoken(&k[0]).into(),
                spoken1: spoken(k1).into(),
                slots: k.len() as i32,
                is_default: km.is_default(*a),
                default_keys: a.defaults().join(" or ").into(),
                // A heading over the first row of each group.
                group: if i == 0 || Action::ALL[i - 1].group() != a.group() { a.group().into() } else { Default::default() },
            }
        })
        .collect();
    // In place when the rows are there: replacing the model would re-create every row,
    // and the key button holding the keyboard focus with it.
    let current = g.get_rows();
    match current.as_any().downcast_ref::<VecModel<KeyRow>>() {
        Some(model) if model.row_count() == rows.len() => {
            for (i, r) in rows.into_iter().enumerate() {
                if model.row_data(i).as_ref() != Some(&r) {
                    model.set_row_data(i, r);
                }
            }
        }
        _ => g.set_rows(ModelRc::new(VecModel::from(rows))),
    }
    if g.get_fixed().row_count() == 0 {
        let fixed: Vec<KeyHelp> = fixed_help().into_iter().map(|(keys, what)| KeyHelp { keys: keys.into(), what: what.into() }).collect();
        g.set_fixed(ModelRc::new(VecModel::from(fixed)));
    }
    g.set_all_default(Action::ALL.iter().all(|a| km.is_default(*a)));
    g.set_play_pause(km.label(Action::PlayPause).into());
    g.set_previous(km.label(Action::Previous).into());
    g.set_next(km.label(Action::Next).into());
    g.set_shuffle(km.label(Action::Shuffle).into());
    g.set_repeat(km.label(Action::Repeat).into());
    g.set_mute(km.label(Action::Mute).into());
    g.set_favourite(km.label(Action::Favourite).into());
    g.set_now_playing(km.label(Action::NowPlaying).into());
    g.set_visualizer(km.label(Action::Visualizer).into());
    g.set_visualizer_look(km.label(Action::VisualizerLook).into());
    g.set_visualizer_place(km.label(Action::VisualizerPlace).into());
    g.set_back(km.label(Action::Back).into());
    g.set_eq_copy(km.label(Action::EqCopy).into());
    g.set_eq_paste(km.label(Action::EqPaste).into());
    g.set_forward(km.label(Action::Forward).into());
    g.set_pin_sidebar(km.label(Action::PinSidebar).into());
    g.set_pin_queue(km.label(Action::PinQueue).into());
}

/// The action and slot waiting for a key, if any.
fn capturing(st: &Rc<State>) -> Option<(usize, Action, usize)> {
    let app = st.app.upgrade()?;
    let g = app.global::<Keys>();
    let row = usize::try_from(g.get_capture_row()).ok()?;
    let a = *Action::ALL.get(row)?;
    let slot = usize::try_from(g.get_capture_slot()).ok().filter(|s| *s < a.slots())?;
    Some((row, a, slot))
}

fn note(st: &Rc<State>, row: i32, text: &str, conflict: bool) {
    let Some(app) = st.app.upgrade() else { return };
    let g = app.global::<Keys>();
    g.set_note_row(row);
    g.set_note(text.into());
    g.set_conflict(conflict);
}

fn end_capture(st: &Rc<State>) {
    PENDING.with(|p| p.borrow_mut().take());
    crate::platform::window::set_capturing(false);
    if let Some(app) = st.app.upgrade() {
        let g = app.global::<Keys>();
        g.set_capture_row(-1);
        g.set_conflict(false);
    }
}

fn save(st: &Rc<State>) {
    st.settings.borrow_mut().shortcuts = st.keymap.borrow().overrides();
    st.save_settings_soon();
    refresh(st);
}

/// Start waiting for a key for `row`'s `slot`.
pub fn capture(st: &Rc<State>, row: i32, slot: i32) {
    let Some(a) = usize::try_from(row).ok().and_then(|r| Action::ALL.get(r).copied()) else { return };
    let Some(app) = st.app.upgrade() else { return };
    PENDING.with(|p| p.borrow_mut().take());
    crate::platform::window::set_capturing(true);
    let g = app.global::<Keys>();
    g.set_capture_row(row);
    g.set_capture_slot(slot.clamp(0, a.slots() as i32 - 1));
    note(st, row, &format!("Press the new key for \u{201c}{}\u{201d}. Esc cancels; Delete leaves it without a key.", a.describe()), false);
}

/// A key pressed while waiting.
pub fn captured(st: &Rc<State>, text: &str, ctrl: bool, shift: bool, alt: bool) {
    let Some((row, a, slot)) = capturing(st) else { return };
    let is = |k: Key| text == char::from(k).to_string();
    let plain = !ctrl && !shift && !alt;
    let pending = PENDING.with(|p| p.borrow().clone());
    if is(Key::Escape) && !ctrl && !alt {
        cancel(st);
        return;
    }
    if is(Key::Return) && plain && pending.is_some() {
        reassign(st);
        return;
    }
    if is(Key::Delete) && plain {
        clear(st);
        return;
    }
    // A lone modifier: wait for the key it goes with.
    let Some(b) = Binding::from_press(text, ctrl, shift, alt) else { return };
    PENDING.with(|p| p.borrow_mut().take());
    let r = st.keymap.borrow_mut().assign(a, slot, b.clone());
    match r {
        Assign::Done | Assign::Unchanged => {
            end_capture(st);
            note(st, row as i32, "", false);
            if r == Assign::Done {
                log::info!("keys: {} is now {}", a.id(), b.config());
                save(st);
            }
        }
        Assign::Fixed(why) => {
            note(st, row as i32, &format!("{} can\u{2019}t be used: {why}. Press another key, or Esc to cancel.", key_name(&b)), false);
        }
        Assign::Conflict(other, _) => {
            PENDING.with(|p| *p.borrow_mut() = Some(b.clone()));
            note(
                st,
                row as i32,
                &format!("{} is the key for \u{201c}{}\u{201d}. Give it to this action instead (that one is left without it)?", key_name(&b), other.describe()),
                true,
            );
        }
    }
}

/// Take the pending key from the action that has it.
pub fn reassign(st: &Rc<State>) {
    let Some((row, a, slot)) = capturing(st) else { return };
    let Some(b) = PENDING.with(|p| p.borrow_mut().take()) else { return };
    let from = st.keymap.borrow().holder(&b).map(|(h, _)| h);
    st.keymap.borrow_mut().reassign(a, slot, b.clone());
    log::info!("keys: {} is now {} (taken from {:?})", a.id(), b.config(), from.map(Action::id));
    end_capture(st);
    let msg = from.map(|h| format!("\u{201c}{}\u{201d} no longer has {}.", h.describe(), key_name(&b))).unwrap_or_default();
    note(st, row as i32, &msg, false);
    save(st);
}

/// Stop waiting (Esc, Cancel, or the focus went elsewhere).
pub fn cancel(st: &Rc<State>) {
    let row = capturing(st).map(|c| c.0 as i32).unwrap_or(-1);
    end_capture(st);
    note(st, row, "", false);
}

/// The page is changing: stop waiting for a key and drop the editor's note.
pub fn leave(st: &Rc<State>) {
    let Some(app) = st.app.upgrade() else { return };
    let g = app.global::<Keys>();
    if g.get_capture_row() >= 0 || !g.get_note().is_empty() {
        end_capture(st);
        note(st, -1, "", false);
    }
}

/// Leave the slot being edited without a key.
pub fn clear(st: &Rc<State>) {
    let Some((row, a, slot)) = capturing(st) else { return };
    st.keymap.borrow_mut().clear(a, slot);
    log::info!("keys: {} slot {slot} cleared", a.id());
    end_capture(st);
    note(st, row as i32, "", false);
    save(st);
}

/// Back to `row`'s default keys (taken back from any action that has one of them).
pub fn reset(st: &Rc<State>, row: i32) {
    let Some(a) = usize::try_from(row).ok().and_then(|r| Action::ALL.get(r).copied()) else { return };
    end_capture(st);
    let lost = st.keymap.borrow_mut().reset(a);
    let msg = lost.iter().map(|(h, b)| format!("\u{201c}{}\u{201d} no longer has {}.", h.describe(), key_name(b))).collect::<Vec<_>>().join(" ");
    note(st, row, &msg, false);
    save(st);
}

pub fn reset_all(st: &Rc<State>) {
    end_capture(st);
    st.keymap.borrow_mut().reset_all();
    note(st, -1, "", false);
    save(st);
}
