//! Windows Ink pen input beside winit. winit 0.30 turns `WM_POINTER` pen input into touch events
//! with a force and drops tilt, rotation, the eraser end and the coalesced moves. A window
//! subclass sees each `WM_POINTER*` message before winit's window procedure, reads the pen's
//! `POINTER_PEN_INFO` (and its history, for moves Windows coalesced) and passes the message on
//! unchanged. Everything read goes through the pure, tested mapping in [`crate::pointer`].
//!
//! The `unsafe` here is Win32 FFI only; every block has a `SAFETY:` comment.

#![allow(unsafe_code)]

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Input::Pointer::{GetPointerPenInfo, GetPointerPenInfoHistory, POINTER_PEN_INFO};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::WM_NCDESTROY;

use crate::pointer::{RawPointer, handle, history, message};
use crate::{Error, Sample, Update, deliver};

/// Subclass id (any value unique among this window's subclasses).
const SUBCLASS_ID: usize = 0x7063_7261;

/// History entries read per message (Windows keeps at most 64).
const MAX_HISTORY: usize = 64;

type Callback = Box<dyn Fn(Option<Sample>)>;

/// Subclass `hwnd` (the window winit created, from the main thread that owns it) so pen input
/// reaches `callback`: `Some(sample)` for pen messages, `None` when the pointer is no pen. The
/// subclass and `callback` live until the window is destroyed.
pub fn install(hwnd: isize, callback: impl Fn(Option<Sample>) + 'static) -> Result<(), Error> {
    let hwnd = hwnd as HWND;
    if hwnd.is_null() {
        return Err(Error::Platform("no window handle".into()));
    }
    let data = Box::into_raw(Box::new(Box::new(callback) as Callback));
    // SAFETY: `hwnd` is a window of this thread (the caller's contract); `subclass_proc` has the
    // `SUBCLASSPROC` signature; `data` is a leaked box that the proc frees on `WM_NCDESTROY`, or
    // that we free below when the call fails and the proc will never run.
    let ok = unsafe { SetWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID, data as usize) };
    if ok == 0 {
        // SAFETY: the subclass was not installed, so nothing else holds `data`.
        drop(unsafe { Box::from_raw(data) });
        return Err(Error::Platform("SetWindowSubclass failed".into()));
    }
    Ok(())
}

/// # Safety
/// Called by comctl32 with `data` the pointer [`install`] leaked.
unsafe extern "system" fn subclass_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, data: usize) -> LRESULT {
    if msg == WM_NCDESTROY {
        // SAFETY: the last message of the window: remove ourselves and free what `install` leaked.
        unsafe {
            RemoveWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID);
            drop(Box::from_raw(data as *mut Callback));
            return DefSubclassProc(hwnd, msg, wparam, lparam);
        }
    }
    if matches!(msg, message::UPDATE | message::DOWN | message::UP | message::LEAVE) {
        // SAFETY: `data` is the live box (freed only above, after which no message arrives).
        let callback: &Callback = unsafe { &*(data as *const Callback) };
        // GET_POINTERID_WPARAM: the low word.
        let id = (wparam & 0xFFFF) as u32;
        for update in read(id, msg) {
            if let Update::Set(sample) = update {
                deliver(callback.as_ref(), sample);
            }
        }
    }
    // SAFETY: forwarding the message we were given to the next procedure in the chain.
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

fn raw(msg: u32, p: &POINTER_PEN_INFO) -> RawPointer {
    RawPointer {
        message: msg,
        kind: p.pointerInfo.pointerType as u32,
        flags: p.pointerInfo.pointerFlags,
        pen_flags: p.penFlags,
        pen_mask: p.penMask,
        pressure: p.pressure,
        rotation: p.rotation,
        tilt_x: p.tiltX,
        tilt_y: p.tiltY,
    }
}

/// The updates of one message: for a move, every sample Windows coalesced (oldest first, ending
/// with this one); otherwise the message's own pen info. A pointer that is no pen (touch) has no
/// pen info: the mapping reads that as "not a pen".
fn read(id: u32, msg: u32) -> Vec<Update> {
    // SAFETY: plain-data out structs; the pointer APIs only write into the buffers we pass.
    unsafe {
        if msg == message::UPDATE {
            let mut count: u32 = 0;
            if GetPointerPenInfoHistory(id, &mut count, std::ptr::null_mut()) != 0 && count > 0 {
                let n = (count as usize).min(MAX_HISTORY);
                let mut buf: Vec<POINTER_PEN_INFO> = vec![std::mem::zeroed(); n];
                let mut count = n as u32;
                if GetPointerPenInfoHistory(id, &mut count, buf.as_mut_ptr()) != 0 {
                    let got = (count as usize).min(n);
                    let entries: Vec<RawPointer> = buf.iter().take(got).map(|p| raw(msg, p)).collect();
                    return history(&entries);
                }
            }
        }
        let mut info: POINTER_PEN_INFO = std::mem::zeroed();
        if GetPointerPenInfo(id, &mut info) != 0 {
            vec![handle(&raw(msg, &info))]
        } else {
            // Touch (no pen info) or an unreadable pointer: not a pen.
            vec![Update::Set(None)]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::Input::Pointer::{POINTER_FLAG_INCONTACT, POINTER_FLAG_INRANGE};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        PEN_FLAG_ERASER, PEN_FLAG_INVERTED, PEN_MASK_PRESSURE, PEN_MASK_ROTATION, PEN_MASK_TILT_X, PEN_MASK_TILT_Y, PT_PEN, WM_POINTERDOWN, WM_POINTERLEAVE,
        WM_POINTERUP, WM_POINTERUPDATE,
    };

    /// The pure mapping's copies of the Win32 constants are the real ones.
    #[test]
    fn constants_match_win32() {
        use crate::pointer as p;
        assert_eq!((message::UPDATE, message::DOWN, message::UP, message::LEAVE), (WM_POINTERUPDATE, WM_POINTERDOWN, WM_POINTERUP, WM_POINTERLEAVE));
        assert_eq!(p::PT_PEN, PT_PEN as u32);
        assert_eq!((p::FLAG_INRANGE, p::FLAG_INCONTACT), (POINTER_FLAG_INRANGE, POINTER_FLAG_INCONTACT));
        assert_eq!((p::PEN_FLAG_INVERTED, p::PEN_FLAG_ERASER), (PEN_FLAG_INVERTED, PEN_FLAG_ERASER));
        assert_eq!(
            (p::PEN_MASK_PRESSURE, p::PEN_MASK_ROTATION, p::PEN_MASK_TILT_X, p::PEN_MASK_TILT_Y),
            (PEN_MASK_PRESSURE, PEN_MASK_ROTATION, PEN_MASK_TILT_X, PEN_MASK_TILT_Y)
        );
    }

    #[test]
    fn struct_fields_map_and_bad_input_fails_gracefully() {
        let mut p: POINTER_PEN_INFO = unsafe { std::mem::zeroed() };
        p.pointerInfo.pointerType = PT_PEN;
        p.pointerInfo.pointerFlags = POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT;
        p.penMask = PEN_MASK_PRESSURE | PEN_MASK_TILT_X;
        p.pressure = 256;
        p.tiltX = 20;
        let Update::Set(Some(s)) = handle(&raw(WM_POINTERUPDATE, &p)) else { panic!("pen") };
        assert_eq!((s.pressure, s.tilt_x), (0.25, 20.0));
        // A pointer id that doesn't exist reads as "not a pen", never a panic.
        assert_eq!(read(0xFFFF, WM_POINTERUPDATE), vec![Update::Set(None)]);
        assert_eq!(read(0, WM_POINTERDOWN), vec![Update::Set(None)]);
        // No window: a graceful error.
        assert!(install(0, |_| {}).is_err());
    }
}
