//! Minimal P6 (binary RGB) PPM reader for the R7 pixel-verification oracle.
//!
//! `opurom-render` writes `P6\n<width> <height>\n255\n` followed by
//! `width * height * 3` RGB bytes. Only that exact shape is accepted; a
//! malformed header is an error value, never a fabricated frame.

use crate::AdapterError;

/// A decoded binary PPM image: RGB8, three bytes per pixel, row-major.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PpmImage {
    pub width: u32,
    pub height: u32,
    /// RGB8 pixels, `width * height * 3` bytes.
    pub pixels: Vec<u8>,
}

/// Decode a binary P6 PPM. Only the exact `P6` shape emitted by the software
/// renderer is supported; comments and other magic numbers are rejected.
pub fn read_ppm(bytes: &[u8]) -> Result<PpmImage, AdapterError> {
    let number = |cursor: &mut usize| -> Result<u32, AdapterError> {
        while *cursor < bytes.len() && bytes[*cursor].is_ascii_whitespace() {
            *cursor += 1;
        }
        let start = *cursor;
        let mut value: u64 = 0;
        while *cursor < bytes.len() && bytes[*cursor].is_ascii_digit() {
            value = value
                .checked_mul(10)
                .and_then(|v| v.checked_add(u64::from(bytes[*cursor] - b'0')))
                .ok_or(AdapterError::Backend("PPM header number overflow"))?;
            *cursor += 1;
        }
        if start == *cursor {
            return Err(AdapterError::Backend("PPM header is malformed"));
        }
        u32::try_from(value).map_err(|_| AdapterError::Backend("PPM header number overflow"))
    };

    if bytes.len() < 2 || &bytes[..2] != b"P6" {
        return Err(AdapterError::Backend("not a binary P6 PPM"));
    }
    let mut cursor = 2usize;
    let width = number(&mut cursor)?;
    let height = number(&mut cursor)?;
    let max_value = number(&mut cursor)?;
    if max_value != 255 {
        return Err(AdapterError::Backend("PPM max value must be 255"));
    }
    if width == 0 || height == 0 {
        return Err(AdapterError::Backend("PPM dimensions must be positive"));
    }
    // Exactly one whitespace byte separates the header from the raster.
    if cursor >= bytes.len() || !bytes[cursor].is_ascii_whitespace() {
        return Err(AdapterError::Backend("PPM header is malformed"));
    }
    cursor += 1;

    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|count| count.checked_mul(3))
        .ok_or(AdapterError::Backend("PPM dimensions overflow"))?;
    let pixels = bytes
        .get(cursor..cursor + expected)
        .ok_or(AdapterError::Backend("PPM raster is truncated"))?
        .to_vec();
    Ok(PpmImage {
        width,
        height,
        pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::read_ppm;
    use crate::AdapterError;

    fn ppm(header: &str, raster: &[u8]) -> Vec<u8> {
        let mut bytes = header.as_bytes().to_vec();
        bytes.extend_from_slice(raster);
        bytes
    }

    #[test]
    fn reads_the_exact_shape_the_software_renderer_writes() {
        let image = read_ppm(&ppm("P6\n2 1\n255\n", &[1, 2, 3, 4, 5, 6])).unwrap();
        assert_eq!(image.width, 2);
        assert_eq!(image.height, 1);
        assert_eq!(image.pixels, vec![1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn rejects_malformed_headers_and_truncated_rasters() {
        assert!(matches!(
            read_ppm(b"P3\n2 1\n255\nabc"),
            Err(AdapterError::Backend(_))
        ));
        assert!(matches!(
            read_ppm(b"P6\n2 1\n256\nabc"),
            Err(AdapterError::Backend(_))
        ));
        assert!(matches!(
            read_ppm(b"P6\n0 1\n255\n"),
            Err(AdapterError::Backend(_))
        ));
        assert!(matches!(
            read_ppm(b"P6\n2 1\n255\nabc"),
            Err(AdapterError::Backend(_))
        ));
        assert!(matches!(
            read_ppm(b"P6\nx 1\n255\n"),
            Err(AdapterError::Backend(_))
        ));
    }
}
