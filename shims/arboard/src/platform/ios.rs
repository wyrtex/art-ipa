/*
SPDX-License-Identifier: Apache-2.0 OR MIT

iOS / iPadOS backend (added by art-ipa): the general `UIPasteboard`.
*/

use std::{
	borrow::Cow,
	path::{Path, PathBuf},
};

use objc2::{
	msg_send,
	rc::Retained,
	runtime::{AnyClass, AnyObject},
};
use objc2_foundation::{NSData, NSString};

#[cfg(feature = "image-data")]
use crate::common::ImageData;
use crate::common::Error;

const TEXT: &str = "public.utf8-plain-text";
const HTML: &str = "public.html";
#[cfg(feature = "image-data")]
const PNG: &str = "public.png";

fn pasteboard() -> Result<Retained<AnyObject>, Error> {
	let cls = AnyClass::get(c"UIPasteboard").ok_or(Error::ClipboardNotSupported)?;
	let pb: Option<Retained<AnyObject>> = unsafe { msg_send![cls, generalPasteboard] };
	pb.ok_or(Error::ClipboardNotSupported)
}

fn data_for(pb: &AnyObject, uti: &str) -> Option<Vec<u8>> {
	let uti = NSString::from_str(uti);
	let data: Option<Retained<NSData>> = unsafe { msg_send![pb, dataForPasteboardType: &*uti] };
	data.map(|d| d.to_vec())
}

/// Replaces the pasteboard's contents with one item holding every `(type, value)` representation.
fn set_item(pb: &AnyObject, reps: &[(&str, &AnyObject)]) -> Result<(), Error> {
	let dict_cls = AnyClass::get(c"NSMutableDictionary").ok_or(Error::ClipboardNotSupported)?;
	let array_cls = AnyClass::get(c"NSArray").ok_or(Error::ClipboardNotSupported)?;
	unsafe {
		let dict: Option<Retained<AnyObject>> = msg_send![dict_cls, new];
		let dict = dict.ok_or_else(|| Error::unknown("NSMutableDictionary"))?;
		for (uti, value) in reps {
			let key = NSString::from_str(uti);
			let _: () = msg_send![&*dict, setObject: *value, forKey: &*key];
		}
		let items: Option<Retained<AnyObject>> = msg_send![array_cls, arrayWithObject: &*dict];
		let items = items.ok_or_else(|| Error::unknown("NSArray"))?;
		let _: () = msg_send![pb, setItems: &*items];
	}
	Ok(())
}

pub(crate) struct Clipboard;

impl Clipboard {
	pub(crate) fn new() -> Result<Clipboard, Error> {
		Ok(Clipboard)
	}
}

pub(crate) struct Get<'clipboard> {
	_clipboard: &'clipboard mut Clipboard,
}

impl<'clipboard> Get<'clipboard> {
	pub(crate) fn new(clipboard: &'clipboard mut Clipboard) -> Self {
		Self { _clipboard: clipboard }
	}

	pub(crate) fn text(self) -> Result<String, Error> {
		let pb = pasteboard()?;
		let s: Option<Retained<NSString>> = unsafe { msg_send![&*pb, string] };
		s.map(|s| s.to_string()).ok_or(Error::ContentNotAvailable)
	}

	pub(crate) fn html(self) -> Result<String, Error> {
		let pb = pasteboard()?;
		let bytes = data_for(&pb, HTML).ok_or(Error::ContentNotAvailable)?;
		String::from_utf8(bytes).map_err(|_| Error::ConversionFailure)
	}

	#[cfg(feature = "image-data")]
	pub(crate) fn image(self) -> Result<ImageData<'static>, Error> {
		let pb = pasteboard()?;
		let bytes = data_for(&pb, PNG).ok_or(Error::ContentNotAvailable)?;
		let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
			.map_err(|_| Error::ConversionFailure)?
			.into_rgba8();
		let (width, height) = img.dimensions();
		Ok(ImageData {
			width: width as usize,
			height: height as usize,
			bytes: Cow::Owned(img.into_raw()),
		})
	}

	pub(crate) fn file_list(self) -> Result<Vec<PathBuf>, Error> {
		Err(Error::ContentNotAvailable)
	}
}

pub(crate) struct Set<'clipboard> {
	_clipboard: &'clipboard mut Clipboard,
}

impl<'clipboard> Set<'clipboard> {
	pub(crate) fn new(clipboard: &'clipboard mut Clipboard) -> Self {
		Self { _clipboard: clipboard }
	}

	pub(crate) fn text(self, data: Cow<'_, str>) -> Result<(), Error> {
		let pb = pasteboard()?;
		let s = NSString::from_str(&data);
		let _: () = unsafe { msg_send![&*pb, setString: &*s] };
		Ok(())
	}

	pub(crate) fn html(self, html: Cow<'_, str>, alt: Option<Cow<'_, str>>) -> Result<(), Error> {
		let pb = pasteboard()?;
		let html = NSString::from_str(&html);
		let alt = alt.map(|a| NSString::from_str(&a));
		let mut reps: Vec<(&str, &AnyObject)> = vec![(HTML, &html)];
		if let Some(alt) = &alt {
			reps.push((TEXT, alt));
		}
		set_item(&pb, &reps)
	}

	#[cfg(feature = "image-data")]
	pub(crate) fn image(self, data: ImageData) -> Result<(), Error> {
		let pb = pasteboard()?;
		let img = image::RgbaImage::from_raw(data.width as u32, data.height as u32, data.bytes.into_owned())
			.ok_or(Error::ConversionFailure)?;
		let mut png = Vec::new();
		img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
			.map_err(|_| Error::ConversionFailure)?;
		let png = NSData::with_bytes(&png);
		set_item(&pb, &[(PNG, &png)])
	}

	pub(crate) fn file_list(self, _file_list: &[impl AsRef<Path>]) -> Result<(), Error> {
		Err(Error::ClipboardNotSupported)
	}
}

pub(crate) struct Clear<'clipboard> {
	_clipboard: &'clipboard mut Clipboard,
}

impl<'clipboard> Clear<'clipboard> {
	pub(crate) fn new(clipboard: &'clipboard mut Clipboard) -> Self {
		Self { _clipboard: clipboard }
	}

	pub(crate) fn clear(self) -> Result<(), Error> {
		let pb = pasteboard()?;
		let array_cls = AnyClass::get(c"NSArray").ok_or(Error::ClipboardNotSupported)?;
		unsafe {
			let empty: Option<Retained<AnyObject>> = msg_send![array_cls, array];
			if let Some(empty) = empty {
				let _: () = msg_send![&*pb, setItems: &*empty];
			}
		}
		Ok(())
	}
}
