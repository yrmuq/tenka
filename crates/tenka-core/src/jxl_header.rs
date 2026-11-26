use std::{mem::MaybeUninit, ptr::NonNull};

use jpegxl_rs::DecodeError;
use jpegxl_sys::{
    color::color_encoding::{JxlColorSpace, JxlPrimaries, JxlTransferFunction, JxlWhitePoint},
    common::types::JxlBool,
    decode::{
        JxlColorProfileTarget, JxlDecoder, JxlDecoderCloseInput, JxlDecoderCreate,
        JxlDecoderDestroy, JxlDecoderGetBasicInfo, JxlDecoderGetColorAsEncodedProfile,
        JxlDecoderProcessInput, JxlDecoderSetInput, JxlDecoderStatus, JxlDecoderSubscribeEvents,
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

    raster::check_dimensions(info.xsize, info.ysize)?;
    if info.have_animation == JxlBool::True {
        return Err(Error::Animation);
    }
    if info.bits_per_sample != 8 || info.exponent_bits_per_sample != 0 {
        return Err(Error::BitDepth);
    }

    // Avoid changing pixel values when the decoder already outputs sRGB.
    if info.uses_original_profile == JxlBool::False {
        return Ok(false);
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
            return Ok(true);
        }
        let encoding = encoding.assume_init();
        Ok(!(encoding.color_space == JxlColorSpace::Rgb
            && encoding.white_point == JxlWhitePoint::D65
            && encoding.primaries == JxlPrimaries::SRgb
            && encoding.transfer_function == JxlTransferFunction::SRGB))
    }
}
