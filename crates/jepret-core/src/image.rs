use crate::geom::Rect;

/// In-memory bitmap used everywhere in Jepret.
///
/// Format is fixed: **RGBA8, sRGB, straight (non-premultiplied) alpha,
/// row-major, stride = `width * 4`**. HDR sources must be tone-mapped into
/// this format by `jepret-capture` before leaving that crate.
#[derive(Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Image")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

impl Image {
    /// Fully transparent image.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; width as usize * height as usize * 4],
        }
    }

    pub fn from_raw(width: u32, height: u32, pixels: Vec<u8>) -> crate::Result<Self> {
        let expected = width as usize * height as usize * 4;
        anyhow::ensure!(
            pixels.len() == expected,
            "pixel buffer is {} bytes, expected {expected} for {width}x{height}",
            pixels.len()
        );
        Ok(Self { width, height, pixels })
    }

    pub fn stride(&self) -> usize {
        self.width as usize * 4
    }

    pub fn row(&self, y: u32) -> &[u8] {
        let s = self.stride();
        &self.pixels[y as usize * s..(y as usize + 1) * s]
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = y as usize * self.stride() + x as usize * 4;
        [self.pixels[i], self.pixels[i + 1], self.pixels[i + 2], self.pixels[i + 3]]
    }

    /// Crop using a rect **relative to this image** (0,0 = top-left).
    /// The rect is clamped to the image bounds.
    pub fn crop(&self, rect: Rect) -> crate::Result<Image> {
        let bounds = Rect::new(0, 0, self.width, self.height);
        let r = bounds
            .intersect(&rect)
            .ok_or_else(|| anyhow::anyhow!("crop rect {rect:?} outside image {bounds:?}"))?;
        let mut out = Vec::with_capacity(r.width as usize * r.height as usize * 4);
        let s = self.stride();
        for y in r.y..r.bottom() {
            let start = y as usize * s + r.x as usize * 4;
            out.extend_from_slice(&self.pixels[start..start + r.width as usize * 4]);
        }
        Image::from_raw(r.width, r.height, out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_clamps() {
        let mut img = Image::new(4, 4);
        img.pixels[(2 * 4 + 3) * 4] = 255; // pixel (3,2) red
        let c = img.crop(Rect::new(2, 2, 10, 10)).unwrap();
        assert_eq!((c.width, c.height), (2, 2));
        assert_eq!(c.pixel(1, 0)[0], 255);
    }
}
