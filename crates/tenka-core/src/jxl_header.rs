use std::{mem::MaybeUninit, ptr::NonNull};

use jpegxl_rs::DecodeError;
use jpegxl_sys::{
    color::color_encoding::{JxlColorSpace, JxlPrimaries, JxlTransferFunction, JxlWhitePoint},
    common::types::{JxlBool, JxlBoxType},
    decode::{
        JxlColorProfileTarget, JxlDecoder, JxlDecoderCloseInput, JxlDecoderCreate,
        JxlDecoderDestroy, JxlDecoderGetBasicInfo, JxlDecoderGetBoxType,
        JxlDecoderGetColorAsEncodedProfile, JxlDecoderProcessInput, JxlDecoderReleaseBoxBuffer,
        JxlDecoderSetBoxBuffer, JxlDecoderSetDecompressBoxes, JxlDecoderSetInput, JxlDecoderStatus,
        JxlDecoderSubscribeEvents,
    },
};

use crate::{Error, Result, raster};

struct HeaderDecoder(NonNull<JxlDecoder>);

impl Drop for HeaderDecoder {
    fn drop(&mut self) {
        // SAFETY: this object owns the decoder returned by JxlDecoderCreate.
        unsafe { JxlDecoderDestroy(self.0.as_ptr()) }
    }
}

pub fn needs_color_conversion(bytes: &[u8]) -> Result<bool> {
    image_header(bytes, crate::MAX_PIXELS).map(|(_, normalize)| normalize)
}

pub fn image_header(bytes: &[u8], max_pixels: u64) -> Result<((u32, u32), bool)> {
    use JxlDecoderStatus::{BasicInfo, ColorEncoding, Success};

    // SAFETY: a null memory manager selects libjxl's default allocator.
    let decoder = HeaderDecoder(
        NonNull::new(unsafe { JxlDecoderCreate(std::ptr::null()) })
            .ok_or(DecodeError::CannotCreateDecoder)?,
    );
    let ptr = decoder.0.as_ptr();
    let mut info = MaybeUninit::uninit();

    // SAFETY: the decoder and input remain live until the header has been read. The
    // information buffer is initialized when JxlDecoderGetBasicInfo returns Success.
    let info = unsafe {
        if JxlDecoderSubscribeEvents(ptr, BasicInfo as i32 | ColorEncoding as i32) != Success
            || JxlDecoderSetInput(ptr, bytes.as_ptr(), bytes.len()) != Success
        {
            return Err(DecodeError::InvalidInput.into());
        }
        JxlDecoderCloseInput(ptr);
        if JxlDecoderProcessInput(ptr) != BasicInfo
            || JxlDecoderGetBasicInfo(ptr, info.as_mut_ptr()) != Success
        {
            return Err(DecodeError::InvalidInput.into());
        }
        info.assume_init()
    };

    raster::check_dimension_limits(info.xsize, info.ysize, max_pixels)?;
    let dimensions = (info.xsize, info.ysize);
    if info.have_animation == JxlBool::True {
        return Err(Error::Animation);
    }
    if info.bits_per_sample != 8 || info.exponent_bits_per_sample != 0 {
        return Err(Error::BitDepth);
    }

    // Avoid changing pixel values when the decoder already outputs sRGB.
    if info.uses_original_profile == JxlBool::False {
        return Ok((dimensions, false));
    }
    let mut encoding = MaybeUninit::uninit();
    // SAFETY: the decoder owns no borrowed pointers beyond this function, and the
    // color-encoding event makes the profile available before pixel decoding.
    unsafe {
        if JxlDecoderProcessInput(ptr) != ColorEncoding {
            return Err(DecodeError::InvalidInput.into());
        }
        if JxlDecoderGetColorAsEncodedProfile(
            ptr,
            JxlColorProfileTarget::Data,
            encoding.as_mut_ptr(),
        ) != Success
        {
            return Ok((dimensions, true));
        }
        let encoding = encoding.assume_init();
        let is_srgb = encoding.color_space == JxlColorSpace::Rgb
            && encoding.white_point == JxlWhitePoint::D65
            && encoding.primaries == JxlPrimaries::SRgb
            && encoding.transfer_function == JxlTransferFunction::SRGB;
        Ok((dimensions, !is_srgb))
    }
}

pub fn exif(bytes: &[u8]) -> Option<Vec<u8>> {
    use JxlDecoderStatus::{Box, Success};

    const MAX_EXIF_BYTES: usize = 2 * 1024 * 1024;
    let mut buffer = Vec::new();
    // SAFETY: the decoder is dropped before its input and optional box buffer.
    let decoder = HeaderDecoder(NonNull::new(unsafe { JxlDecoderCreate(std::ptr::null()) })?);
    let ptr = decoder.0.as_ptr();
    // SAFETY: the input remains valid and the output buffer is never resized
    // while libjxl owns it.
    unsafe {
        if JxlDecoderSubscribeEvents(ptr, Box as i32) != Success
            || JxlDecoderSetDecompressBoxes(ptr, JxlBool::True) != Success
            || JxlDecoderSetInput(ptr, bytes.as_ptr(), bytes.len()) != Success
        {
            return None;
        }
        JxlDecoderCloseInput(ptr);
        loop {
            let event = JxlDecoderProcessInput(ptr);
            if !buffer.is_empty() && matches!(event, Box | Success) {
                let unused = JxlDecoderReleaseBoxBuffer(ptr);
                buffer.truncate(buffer.len().checked_sub(unused)?);
                let offset = u32::from_be_bytes(buffer.get(..4)?.try_into().ok()?);
                let offset = (offset as usize).checked_add(4)?;
                return Some(buffer.get(offset..)?.to_vec());
            }
            if event != Box {
                return None;
            }
            let mut box_type = JxlBoxType([0; 4]);
            if JxlDecoderGetBoxType(ptr, &mut box_type, JxlBool::True) == Success
                && box_type.0.map(|byte| byte.to_ne_bytes()[0]) == *b"Exif"
            {
                buffer.resize(MAX_EXIF_BYTES, 0);
                if JxlDecoderSetBoxBuffer(ptr, buffer.as_mut_ptr(), buffer.len()) != Success {
                    return None;
                }
            }
        }
    }
}
