//! Converts DXGI pointer shapes into textures the renderer can draw.
//!
//! The renderer draws `color` with normal alpha blending. Pixels that Windows draws by
//! inverting the screen (the text I-beam, for example) go into `invert`, which is drawn with an
//! inverting blend so the cursor stays visible on both dark and light backgrounds.

/// DXGI_OUTDUPL_POINTER_SHAPE_TYPE values.
pub const SHAPE_MONOCHROME: u32 = 1;
pub const SHAPE_COLOR: u32 = 2;
pub const SHAPE_MASKED_COLOR: u32 = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorImage {
    pub width: u32,
    pub height: u32,
    /// BGRA, straight alpha, `width * 4` bytes per row.
    pub color: Vec<u8>,
    /// BGRA inversion mask (white inverts, black leaves the screen alone), if any pixel inverts.
    pub invert: Option<Vec<u8>>,
}

/// `height` is the height DXGI reports; for monochrome shapes it covers both masks.
pub fn convert(kind: u32, width: u32, height: u32, pitch: u32, data: &[u8]) -> Option<CursorImage> {
    let (w, h, p) = (width as usize, height as usize, pitch as usize);
    if w == 0 || h == 0 {
        return None;
    }
    match kind {
        SHAPE_MONOCHROME => {
            let h = h / 2;
            if h == 0 || p * h * 2 > data.len() || p * 8 < w {
                return None;
            }
            let bit = |row: usize, col: usize| data[row * p + col / 8] & (0x80 >> (col % 8)) != 0;
            let mut color = vec![0u8; w * h * 4];
            let mut invert = vec![0u8; w * h * 4];
            let mut any_invert = false;
            for y in 0..h {
                for x in 0..w {
                    let i = (y * w + x) * 4;
                    match (bit(y, x), bit(y + h, x)) {
                        (false, false) => color[i..i + 4].copy_from_slice(&[0, 0, 0, 255]),
                        (false, true) => color[i..i + 4].copy_from_slice(&[255, 255, 255, 255]),
                        (true, false) => {}
                        (true, true) => {
                            invert[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
                            any_invert = true;
                        }
                    }
                }
            }
            Some(CursorImage { width, height: h as u32, color, invert: any_invert.then_some(invert) })
        }
        SHAPE_COLOR | SHAPE_MASKED_COLOR => {
            if p < w * 4 || p * (h - 1) + w * 4 > data.len() {
                return None;
            }
            let mut color = vec![0u8; w * h * 4];
            let mut invert = vec![0u8; w * h * 4];
            let mut any_invert = false;
            for y in 0..h {
                for x in 0..w {
                    let s = y * p + x * 4;
                    let d = (y * w + x) * 4;
                    let px = &data[s..s + 4];
                    if kind == SHAPE_COLOR {
                        color[d..d + 4].copy_from_slice(px);
                    } else if px[3] == 0 {
                        // Mask bit clear: replace the screen pixel with this color.
                        color[d..d + 4].copy_from_slice(&[px[0], px[1], px[2], 255]);
                    } else if px[0] | px[1] | px[2] != 0 {
                        // Mask bit set: XOR with the screen, drawn as an inversion.
                        invert[d..d + 4].copy_from_slice(&[px[0], px[1], px[2], 255]);
                        any_invert = true;
                    }
                }
            }
            Some(CursorImage { width, height, color, invert: any_invert.then_some(invert) })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monochrome_masks_map_to_four_pixel_kinds() {
        // 4x1 cursor, pitch 1 byte. AND row then XOR row.
        // Pixels: (and,xor) = (0,0) black, (0,1) white, (1,0) transparent, (1,1) invert.
        let and_row = 0b0011_0000u8;
        let xor_row = 0b0101_0000u8;
        let img = convert(SHAPE_MONOCHROME, 4, 2, 1, &[and_row, xor_row]).unwrap();
        assert_eq!((img.width, img.height), (4, 1));
        assert_eq!(&img.color[0..4], &[0, 0, 0, 255]);
        assert_eq!(&img.color[4..8], &[255, 255, 255, 255]);
        assert_eq!(&img.color[8..12], &[0, 0, 0, 0]);
        assert_eq!(&img.color[12..16], &[0, 0, 0, 0]);
        let inv = img.invert.unwrap();
        assert_eq!(&inv[12..16], &[255, 255, 255, 255]);
        assert_eq!(&inv[0..12], &[0; 12]);
    }

    #[test]
    fn color_cursor_respects_pitch() {
        // 1x2 cursor with 8-byte pitch (4 bytes of padding per row).
        let data = [1, 2, 3, 4, 9, 9, 9, 9, 5, 6, 7, 8, 9, 9, 9, 9];
        let img = convert(SHAPE_COLOR, 1, 2, 8, &data).unwrap();
        assert_eq!(img.color, vec![1, 2, 3, 4, 5, 6, 7, 8]);
        assert!(img.invert.is_none());
    }

    #[test]
    fn masked_color_splits_replace_and_xor() {
        let data = [
            10, 20, 30, 0, // replace
            0, 0, 0, 255, // xor with black: no-op
            255, 255, 255, 255, // xor with white: invert
        ];
        let img = convert(SHAPE_MASKED_COLOR, 3, 1, 12, &data).unwrap();
        assert_eq!(&img.color[0..4], &[10, 20, 30, 255]);
        assert_eq!(&img.color[4..12], &[0; 8]);
        let inv = img.invert.unwrap();
        assert_eq!(&inv[8..12], &[255, 255, 255, 255]);
        assert_eq!(&inv[0..8], &[0; 8]);
    }

    #[test]
    fn rejects_truncated_or_unknown_shapes() {
        assert!(convert(SHAPE_COLOR, 2, 2, 8, &[0; 10]).is_none());
        assert!(convert(SHAPE_MONOCHROME, 16, 2, 1, &[0; 2]).is_none());
        assert!(convert(SHAPE_MONOCHROME, 8, 1, 1, &[0; 2]).is_none());
        assert!(convert(99, 1, 1, 4, &[0; 4]).is_none());
        assert!(convert(SHAPE_COLOR, 0, 1, 4, &[0; 4]).is_none());
    }
}
