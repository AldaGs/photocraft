//! Windows Ink `POINTER_PEN_INFO` fields → [`Sample`]s. Pure (no Win32 calls), so it is tested on
//! every platform; `windows.rs` reads the fields and feeds them here.
//!
//! - Pressure is 0..1024 (when `PEN_MASK_PRESSURE` is set; otherwise a contact is full pressure,
//!   like a mouse, and a hover is 0), tilt is -90..90 degrees per axis, rotation 0..359 degrees
//!   clockwise.
//! - The eraser end is `PEN_FLAG_ERASER` (the eraser is touching) or `PEN_FLAG_INVERTED` (it is
//!   in range), so the tool switches as the pen flips, before it touches (#2285).
//! - A pen's `WM_POINTERUP` reports pressure 0 and keeps the sample instead: it reaches the UI in
//!   the same frame as the stroke's last moves, which must keep the pressure they were drawn with
//!   (as in [`crate::appkit`]).
//! - Windows coalesces pointer moves when the app is slow; `GetPointerPenInfoHistory` returns the
//!   skipped ones, newest first. [`history`] turns them into updates in time order, so every
//!   sample reaches the brush instead of only the newest one (#2046).

use crate::{Sample, Update};

/// Window messages (winuser.h).
pub mod message {
    pub const UPDATE: u32 = 0x0245;
    pub const DOWN: u32 = 0x0246;
    pub const UP: u32 = 0x0247;
    pub const LEAVE: u32 = 0x024A;
}

/// `POINTER_INPUT_TYPE` of a pen.
pub const PT_PEN: u32 = 3;
/// `POINTER_FLAG_INRANGE`.
pub const FLAG_INRANGE: u32 = 0x2;
/// `POINTER_FLAG_INCONTACT`.
pub const FLAG_INCONTACT: u32 = 0x4;
/// `PEN_FLAG_INVERTED`, `PEN_FLAG_ERASER`.
pub const PEN_FLAG_INVERTED: u32 = 0x2;
pub const PEN_FLAG_ERASER: u32 = 0x4;
/// `PEN_MASK_*`: which of the optional fields are valid.
pub const PEN_MASK_PRESSURE: u32 = 0x1;
pub const PEN_MASK_ROTATION: u32 = 0x2;
pub const PEN_MASK_TILT_X: u32 = 0x4;
pub const PEN_MASK_TILT_Y: u32 = 0x8;

/// The fields of one pointer message / history entry, read as plain values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RawPointer {
    /// `message::*` (anything else is ignored).
    pub message: u32,
    /// `POINTER_INPUT_TYPE`.
    pub kind: u32,
    /// `POINTER_INFO::pointerFlags`.
    pub flags: u32,
    pub pen_flags: u32,
    pub pen_mask: u32,
    pub pressure: u32,
    pub rotation: u32,
    pub tilt_x: i32,
    pub tilt_y: i32,
}

/// Map one pointer message.
pub fn handle(e: &RawPointer) -> Update {
    if !matches!(e.message, message::UPDATE | message::DOWN | message::UP | message::LEAVE) {
        return Update::Keep;
    }
    // Fingers and the leaving pointer are no pen; so is a pen that went out of range.
    if e.message == message::LEAVE || e.kind != PT_PEN || e.flags & FLAG_INRANGE == 0 {
        return Update::Set(None);
    }
    if e.message == message::UP {
        return Update::Keep;
    }
    let has = |m: u32| e.pen_mask & m != 0;
    let pressure = match (e.flags & FLAG_INCONTACT != 0, has(PEN_MASK_PRESSURE)) {
        (false, _) => 0.0,
        (true, true) => e.pressure as f32 / 1024.0,
        (true, false) => 1.0,
    };
    Update::Set(Some(
        Sample {
            pressure,
            tilt_x: if has(PEN_MASK_TILT_X) { e.tilt_x as f32 } else { 0.0 },
            tilt_y: if has(PEN_MASK_TILT_Y) { e.tilt_y as f32 } else { 0.0 },
            rotation: if has(PEN_MASK_ROTATION) { e.rotation as f32 } else { 0.0 },
            eraser: e.pen_flags & (PEN_FLAG_ERASER | PEN_FLAG_INVERTED) != 0,
        }
        .sanitized(),
    ))
}

/// A pointer's history as returned by `GetPointerPenInfoHistory` (newest first) as the updates to
/// deliver, oldest first.
pub fn history(newest_first: &[RawPointer]) -> Vec<Update> {
    newest_first.iter().rev().map(handle).filter(|u| *u != Update::Keep).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pen(message: u32) -> RawPointer {
        RawPointer {
            message,
            kind: PT_PEN,
            flags: FLAG_INRANGE | FLAG_INCONTACT,
            pen_mask: PEN_MASK_PRESSURE | PEN_MASK_ROTATION | PEN_MASK_TILT_X | PEN_MASK_TILT_Y,
            ..Default::default()
        }
    }

    fn sample(u: Update) -> Sample {
        match u {
            Update::Set(Some(s)) => s,
            other => panic!("expected a sample, got {other:?}"),
        }
    }

    #[test]
    fn pen_fields_map_to_a_sample() {
        let e = RawPointer { pressure: 512, tilt_x: -30, tilt_y: 45, rotation: 90, ..pen(message::UPDATE) };
        assert_eq!(sample(handle(&e)), Sample { pressure: 0.5, tilt_x: -30.0, tilt_y: 45.0, rotation: 90.0, eraser: false });
        let e = RawPointer { pressure: 1024, tilt_x: 90, tilt_y: -90, rotation: 359, ..pen(message::DOWN) };
        let s = sample(handle(&e));
        assert_eq!((s.pressure, s.tilt_x, s.tilt_y, s.rotation), (1.0, 90.0, -90.0, 359.0));
        // Rotation wraps.
        assert_eq!(sample(handle(&RawPointer { rotation: 360, ..pen(message::UPDATE) })).rotation, 0.0);
    }

    #[test]
    fn eraser_end_by_either_flag() {
        for f in [PEN_FLAG_ERASER, PEN_FLAG_INVERTED, PEN_FLAG_ERASER | PEN_FLAG_INVERTED] {
            assert!(sample(handle(&RawPointer { pen_flags: f, ..pen(message::UPDATE) })).eraser, "{f}");
        }
        assert!(!sample(handle(&pen(message::UPDATE))).eraser);
        // Hovering inverted: the eraser switch happens before it touches.
        let hover = RawPointer { flags: FLAG_INRANGE, pen_flags: PEN_FLAG_INVERTED, pressure: 700, ..pen(message::UPDATE) };
        let s = sample(handle(&hover));
        assert!(s.eraser && s.pressure == 0.0);
    }

    #[test]
    fn missing_fields_default_like_a_mouse_contact() {
        let e = RawPointer { pen_mask: 0, pressure: 999, tilt_x: 40, rotation: 40, ..pen(message::UPDATE) };
        assert_eq!(sample(handle(&e)), Sample::default());
        let hover = RawPointer { pen_mask: 0, flags: FLAG_INRANGE, ..pen(message::UPDATE) };
        assert_eq!(sample(handle(&hover)).pressure, 0.0);
        // A light tap below the first pressure level is pressure 0, not a mouse click.
        assert_eq!(sample(handle(&RawPointer { pressure: 0, ..pen(message::DOWN) })).pressure, 0.0);
    }

    #[test]
    fn lift_keeps_the_stroke_and_leaving_is_a_mouse() {
        assert_eq!(handle(&pen(message::UP)), Update::Keep);
        assert_eq!(handle(&pen(message::LEAVE)), Update::Set(None));
        assert_eq!(handle(&RawPointer { flags: 0, ..pen(message::UPDATE) }), Update::Set(None), "out of range");
        assert_eq!(handle(&RawPointer { kind: 2, ..pen(message::UPDATE) }), Update::Set(None), "touch");
        assert_eq!(handle(&RawPointer { message: 0x0200, ..pen(message::UPDATE) }), Update::Keep, "WM_MOUSEMOVE");
    }

    #[test]
    fn history_is_delivered_oldest_first() {
        let at = |p| RawPointer { pressure: p, ..pen(message::UPDATE) };
        let newest_first = [at(900), at(600), RawPointer { message: message::UP, ..at(0) }, at(300)];
        let got: Vec<f32> = history(&newest_first).into_iter().map(|u| sample(u).pressure).collect();
        assert_eq!(got, vec![300.0 / 1024.0, 600.0 / 1024.0, 900.0 / 1024.0]);
        assert!(history(&[]).is_empty());
    }

    #[test]
    fn garbage_never_panics_and_stays_in_range() {
        for v in [0, 1, 1023, 1024, 4096, u32::MAX] {
            for t in [i32::MIN, -91, 0, 91, i32::MAX] {
                let e = RawPointer { pressure: v, rotation: v, tilt_x: t, tilt_y: t, pen_mask: u32::MAX, pen_flags: u32::MAX, ..pen(message::UPDATE) };
                let s = sample(handle(&e));
                assert!((0.0..=1.0).contains(&s.pressure));
                assert!((-90.0..=90.0).contains(&s.tilt_x) && (-90.0..=90.0).contains(&s.tilt_y));
                assert!((0.0..360.0).contains(&s.rotation));
            }
        }
        assert_eq!(handle(&RawPointer::default()), Update::Keep);
    }
}
