//! Pixel rows from Media Foundation to the JPEG encoder.

/// RGB rows, top first, from 32-bit BGRX rows `stride` bytes apart. A
/// negative stride is Media Foundation's bottom-up picture: the buffer
/// starts with the bottom row.
pub fn bgrx_to_rgb(src: &[u8], width: u32, height: u32, stride: i32) -> Option<Vec<u8>> {
    let (w, h) = (width as usize, height as usize);
    let pitch = stride.unsigned_abs() as usize;
    if pitch < w * 4 || src.len() < pitch * h.saturating_sub(1) + w * 4 {
        return None;
    }
    let mut out = Vec::with_capacity(w * h * 3);
    for row in 0..h {
        let at = if stride < 0 { (h - 1 - row) * pitch } else { row * pitch };
        for px in src[at..at + w * 4].chunks_exact(4) {
            out.extend_from_slice(&[px[2], px[1], px[0]]);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2 x 2: top row red, green; bottom row blue, white. BGRX, 12-byte rows
    // (4 bytes of padding each).
    const TOP: [u8; 12] = [0, 0, 255, 0, 0, 255, 0, 0, 9, 9, 9, 9];
    const BOTTOM: [u8; 12] = [255, 0, 0, 0, 255, 255, 255, 0, 9, 9, 9, 9];
    const RGB: [u8; 12] = [255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255];

    #[test]
    fn top_down_rows_keep_their_order() {
        let src = [TOP, BOTTOM].concat();
        assert_eq!(bgrx_to_rgb(&src, 2, 2, 12).unwrap(), RGB);
    }

    #[test]
    fn bottom_up_rows_are_turned_over() {
        let src = [BOTTOM, TOP].concat();
        assert_eq!(bgrx_to_rgb(&src, 2, 2, -12).unwrap(), RGB);
    }

    #[test]
    fn a_short_buffer_is_refused() {
        assert!(bgrx_to_rgb(&[0; 15], 2, 2, 12).is_none());
        assert!(bgrx_to_rgb(&[0; 24], 2, 2, 4).is_none());
    }
}
