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

/// `src` (RGB, `w` x `h`) shrunk to `tw` wide, the height to the aspect (at
/// least 1): each output pixel is the mean of the source pixels it covers.
pub fn downscale_rgb(src: &[u8], w: u32, h: u32, tw: u32) -> (Vec<u8>, u32, u32) {
    let tw = tw.clamp(1, w.max(1));
    let th = ((h as f64 * tw as f64 / w.max(1) as f64).round() as u32).max(1);
    let (w, h, tw_, th_) = (w as usize, h as usize, tw as usize, th as usize);
    let span = |o: usize, n: usize, of: usize| {
        let a = o * n / of;
        (a, ((o + 1) * n / of).max(a + 1).min(n))
    };
    let mut out = Vec::with_capacity(tw_ * th_ * 3);
    for oy in 0..th_ {
        let (y0, y1) = span(oy, h, th_);
        for ox in 0..tw_ {
            let (x0, x1) = span(ox, w, tw_);
            let mut sum = [0u64; 3];
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = (y * w + x) * 3;
                    for (c, s) in sum.iter_mut().enumerate() {
                        *s += src[i + c] as u64;
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u64;
            out.extend(sum.iter().map(|s| ((s + n / 2) / n) as u8));
        }
    }
    (out, tw, th)
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
    fn downscale_halves_a_checkerboard() {
        // 4 x 2: left half red, right half blue
        let px = |c: [u8; 3]| c.to_vec();
        let row = [px([255, 0, 0]), px([255, 0, 0]), px([0, 0, 255]), px([0, 0, 255])].concat();
        let src = [row.clone(), row].concat();
        let (out, w, h) = downscale_rgb(&src, 4, 2, 2);
        assert_eq!((w, h), (2, 1));
        assert_eq!(out, vec![255, 0, 0, 0, 0, 255]);
    }

    #[test]
    fn downscale_keeps_the_aspect() {
        let src = vec![100u8; 1280 * 720 * 3];
        let (out, w, h) = downscale_rgb(&src, 1280, 720, 320);
        assert_eq!((w, h), (320, 180));
        assert_eq!(out.len(), 320 * 180 * 3);
        assert!(out.iter().all(|&v| v == 100));
    }

    #[test]
    fn a_short_buffer_is_refused() {
        assert!(bgrx_to_rgb(&[0; 15], 2, 2, 12).is_none());
        assert!(bgrx_to_rgb(&[0; 24], 2, 2, 4).is_none());
    }
}
