//! Keeps the stream display out of the way. Windows only allows layouts where every display
//! shares an edge with another, so the stream display can't float apart from the real
//! monitors. What must never happen is the stream display sitting *between* two real monitors:
//! the mouse guard would then stop the cursor from crossing between them. When that happens it
//! is moved to the right of the rightmost monitor, and Windows closes the gap it leaves.

use crate::geometry::Rect;
use windows::Win32::Graphics::Gdi::{
    CDS_NORESET, CDS_UPDATEREGISTRY, ChangeDisplaySettingsExW, DEVMODEW, DISP_CHANGE_SUCCESSFUL, DM_POSITION,
    ENUM_CURRENT_SETTINGS, EnumDisplaySettingsW,
};
use windows::core::{HSTRING, PCWSTR};

/// True if the two rectangles share a stretch of edge (corners alone don't connect).
pub fn touching(a: &Rect, b: &Rect) -> bool {
    let v_overlap = a.top.max(b.top) < a.bottom.min(b.bottom);
    let h_overlap = a.left.max(b.left) < a.right.min(b.right);
    ((a.right == b.left || b.right == a.left) && v_overlap)
        || ((a.bottom == b.top || b.bottom == a.top) && h_overlap)
}

/// True if the real monitors only connect to each other through the stream display, i.e. it
/// sits between them and would block the mouse.
pub fn stream_blocks_path(physical: &[Rect]) -> bool {
    if physical.len() < 2 {
        return false;
    }
    let mut reached = vec![false; physical.len()];
    let mut stack = vec![0];
    reached[0] = true;
    while let Some(i) = stack.pop() {
        for j in 0..physical.len() {
            if !reached[j] && touching(&physical[i], &physical[j]) {
                reached[j] = true;
                stack.push(j);
            }
        }
    }
    reached.iter().any(|r| !r)
}

/// Where to put the stream display: right of the rightmost monitor, aligned with its top.
pub fn edge_position(physical: &[Rect]) -> Option<(i32, i32)> {
    physical.iter().max_by_key(|r| (r.right, -r.top)).map(|r| (r.right, r.top))
}

/// Moves a display on the desktop (no admin rights needed; same as dragging it in Settings).
pub fn move_display(gdi_name: &str, x: i32, y: i32) -> Result<(), String> {
    let name = HSTRING::from(gdi_name);
    let mut mode = DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() };
    // SAFETY: valid name and DEVMODEW with dmSize set; the change is applied with a final call.
    unsafe {
        if !EnumDisplaySettingsW(&name, ENUM_CURRENT_SETTINGS, &mut mode).as_bool() {
            return Err(format!("can't read settings of {gdi_name}"));
        }
        mode.dmFields = DM_POSITION;
        mode.Anonymous1.Anonymous2.dmPosition.x = x;
        mode.Anonymous1.Anonymous2.dmPosition.y = y;
        let r = ChangeDisplaySettingsExW(&name, Some(&mode), None, CDS_UPDATEREGISTRY | CDS_NORESET, None);
        if r != DISP_CHANGE_SUCCESSFUL {
            return Err(format!("moving {gdi_name} failed ({})", r.0));
        }
        let r = ChangeDisplaySettingsExW(PCWSTR::null(), None, None, Default::default(), None);
        if r != DISP_CHANGE_SUCCESSFUL {
            return Err(format!("applying the new layout failed ({})", r.0));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAIN: Rect = Rect::new(0, 0, 2560, 1440);

    #[test]
    fn touching_needs_a_shared_edge() {
        assert!(touching(&MAIN, &Rect::new(2560, 0, 4480, 1080)));
        assert!(touching(&MAIN, &Rect::new(-1920, 300, 0, 1380)));
        assert!(touching(&MAIN, &Rect::new(0, 1440, 1920, 2520)));
        // Corner only.
        assert!(!touching(&MAIN, &Rect::new(2560, 1440, 4480, 2520)));
        // Gap.
        assert!(!touching(&MAIN, &Rect::new(4480, 0, 6400, 1080)));
    }

    #[test]
    fn detects_stream_between_monitors() {
        // Main | stream (2560..4480) | second monitor: the two real monitors don't touch.
        assert!(stream_blocks_path(&[MAIN, Rect::new(4480, 0, 6400, 1080)]));
        // Second monitor right next to main: fine wherever the stream display is.
        assert!(!stream_blocks_path(&[MAIN, Rect::new(2560, 0, 4480, 1080)]));
        // Three monitors in a row, all touching.
        assert!(!stream_blocks_path(&[Rect::new(-1920, 0, 0, 1080), MAIN, Rect::new(2560, 0, 4480, 1080)]));
        assert!(!stream_blocks_path(&[MAIN]));
        assert!(!stream_blocks_path(&[]));
    }

    #[test]
    fn edge_position_is_right_of_rightmost() {
        assert_eq!(edge_position(&[MAIN, Rect::new(-1920, 0, 0, 1080)]), Some((2560, 0)));
        assert_eq!(edge_position(&[MAIN, Rect::new(2560, 200, 4480, 1280)]), Some((4480, 200)));
        assert_eq!(edge_position(&[]), None);
    }
}
