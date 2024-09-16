use crate::error::{ParsedError, ParsingError};
use crate::file::Mime;
use crate::parser::{BufParser, ParsingState};
use crate::skip::Skip;
use crate::slice::SubsliceRange;
use crate::{heif, jpeg, MediaParser, MediaSource, ZB};
#[allow(deprecated)]
use crate::{input::Input, FileFormat};
pub use exif_exif::Exif;
use exif_iter::ImageFileDirectoryIter;
pub use exif_iter::{ExifIter, ParsedExifEntry};
pub use gps::{GPSInfo, LatLng};
pub use tags::ExifTag;

use std::fmt::Debug;
use std::io::Read;
use std::ops::Range;

pub(crate) mod ifd;
pub(crate) use exif_exif::{check_exif_header, TiffHeader};
pub(crate) use exif_iter::IFDHeaderIter;

mod exif_exif;
mod exif_iter;
mod gps;
mod tags;

/// *Deprecated*: Please use [`crate::MediaParser`] instead.
///
/// Read exif data from `reader`, and build an [`ExifIter`] for it.
///
/// ~~If `format` is None, the parser will detect the file format automatically.~~
/// *The `format` param will be ignored from v2.0.0.*
///
/// Currently supported file formats are:
///
/// - *.heic, *.heif, etc.
/// - *.jpg, *.jpeg, etc.
///
/// *.tiff/*.tif is not supported by this function, please use `MediaParser`
/// instead.
///
/// All entries are lazy-parsed. That is, only when you iterate over
/// [`ExifIter`] will the IFD entries be parsed one by one.
///
/// The one exception is the time zone entries. The parser will try to find and
/// parse the time zone data first, so we can correctly parse all time
/// information in subsequent iterates.
///
/// Please note that the parsing routine itself provides a buffer, so the
/// `reader` may not need to be wrapped with `BufRead`.
///
/// Returns:
///
/// - An `Ok<Some<ExifIter>>` if Exif data is found and parsed successfully.
/// - An `Ok<None>` if Exif data is not found.
/// - An `Err` if Exif data is found but parsing failed.
#[deprecated(since = "2.0.0")]
#[allow(deprecated)]
pub fn parse_exif<T: Read>(reader: T, _: Option<FileFormat>) -> crate::Result<Option<ExifIter>> {
    let mut parser = MediaParser::new();
    let iter: ExifIter = parser.parse(MediaSource::unseekable(reader)?)?;
    let iter = iter.to_owned();
    Ok(Some(iter))
}

#[tracing::instrument]
pub(crate) fn parse_exif_iter<R: Read, S: Skip<R>>(
    parser: &mut MediaParser,
    mut ms: MediaSource<R, S>,
) -> Result<ExifIter, crate::Error> {
    let out = parser.load_and_parse::<R, S, _, Option<(Range<_>, Option<ParsingState>)>>(
        ms.reader.by_ref(),
        |buf, state| match ms.mime {
            Mime::Image(img) => {
                tracing::debug!(parsing_buf_len = buf.len(), ?state);
                let exif_data = extract_exif_with_mime(img, buf, state.as_ref())?;
                Ok(exif_data
                    .and_then(|x| buf.subslice_range(x))
                    .map(|x| (x, state)))
            }
            Mime::Video(_) => Err("not an image".into()),
        },
    )?;

    if let Some((range, state)) = out {
        tracing::debug!(?range);
        let input: Input = Input::new(parser.share_buf(), range);
        let iter = input_into_iter(input, state)?;
        Ok(iter)
    } else {
        Err("parse exif failed".into())
    }
}

#[cfg(feature = "async")]
#[tracing::instrument]
pub(crate) async fn parse_exif_iter_async<
    R: AsyncRead + Unpin + Send,
    S: crate::skip::AsyncSkip<R>,
>(
    parser: &mut crate::AsyncMediaParser,
    mut ms: crate::AsyncMediaSource<R, S>,
) -> Result<ExifIter, crate::Error> {
    use crate::parser_async::AsyncBufParser;

    let out = parser
        .load_and_parse::<R, S, _, Option<(Range<_>, Option<ParsingState>)>>(
            &mut ms.reader,
            |buf, state| match ms.mime {
                Mime::Image(img) => {
                    tracing::debug!(parsing_buf_len = buf.len(), ?state);
                    let exif_data = extract_exif_with_mime(img, buf, state.as_ref())?;
                    Ok(exif_data
                        .and_then(|x| buf.subslice_range(x))
                        .map(|x| (x, state)))
                }
                Mime::Video(_) => Err("not an image".into()),
            },
        )
        .await?;

    if let Some((range, state)) = out {
        tracing::debug!(?range);
        let input: Input = Input::new(parser.share_buf(), range);
        let iter = input_into_iter(input, state)?;
        Ok(iter)
    } else {
        Err("parse exif failed".into())
    }
}

pub(crate) fn extract_exif_with_mime<'a>(
    img_type: crate::file::MimeImage,
    buf: &'a [u8],
    state: Option<&ParsingState>,
) -> Result<Option<&'a [u8]>, ParsingError> {
    let (_, exif_data) = match img_type {
        crate::file::MimeImage::Jpeg => jpeg::extract_exif_data(buf)?,
        crate::file::MimeImage::Heic | crate::file::MimeImage::Heif => {
            heif::extract_exif_data(buf)?
        }
        crate::file::MimeImage::Tiff => {
            let (header, data_start) = match state.as_ref() {
                Some(ParsingState::TiffHeader(h)) => (h.to_owned(), 0),
                None => {
                    let (_, header) = TiffHeader::parse(buf)?;
                    if header.ifd0_offset as usize > buf.len() {
                        return Err(ParsingError::ClearAndSkip(
                            header.ifd0_offset as usize,
                            Some(ParsingState::TiffHeader(header)),
                        ));
                    }
                    let start = header.ifd0_offset as usize;
                    (header, start)
                }
            };

            // full fill TIFF data
            let mut iter =
                IFDHeaderIter::new(&buf[data_start..], header.ifd0_offset, header.endian);
            iter.parse_ifd_header(0)?;

            (ZB, Some(buf))
        }
    };
    Ok(exif_data)
}

#[cfg(feature = "async")]
use tokio::io::AsyncRead;

/// *Deprecated*: Please use [`crate::MediaParser`] instead.
///
/// `async` version of [`parse_exif`].
#[allow(deprecated)]
#[cfg(feature = "async")]
#[deprecated(since = "2.0.0")]
pub async fn parse_exif_async<T: AsyncRead + Unpin + Send>(
    reader: T,
    _: Option<FileFormat>,
) -> crate::Result<Option<ExifIter>> {
    use crate::{AsyncMediaParser, AsyncMediaSource};

    let mut parser = AsyncMediaParser::new();
    let exif: ExifIter = parser
        .parse(AsyncMediaSource::unseekable(reader).await?)
        .await?;
    Ok(Some(exif))
}

/// Parses header from input data, and returns an [`ExifIter`].
///
/// All entries are lazy-parsed. That is, only when you iterate over
/// [`ExifIter`] will the IFD entries be parsed one by one.
///
/// The one exception is the time zone entries. The method will try to find
/// and parse the time zone data first, so we can correctly parse all time
/// information in subsequent iterates.
#[tracing::instrument]
pub(crate) fn input_into_iter(
    input: impl Into<Input> + Debug,
    state: Option<ParsingState>,
) -> Result<ExifIter, ParsedError> {
    let iter = input_to_iter(input.into(), state).map_err(|e| match e {
        ParsingError::Need(_) => {
            debug_assert!(false, "input_into_iter got: {e:?}");
            tracing::error!(?e, "input_into_iter error");
            ParsedError::NoEnoughBytes
        }
        ParsingError::ClearAndSkip(_, _) => {
            debug_assert!(false, "input_into_iter got: {e:?}");
            tracing::error!(?e, "input_into_iter error");
            ParsedError::Failed("recv ClearAndSkip".into())
        }
        ParsingError::Failed(v) => ParsedError::Failed(v),
    })?;
    Ok(iter)
}

#[tracing::instrument]
fn input_to_iter(input: Input, state: Option<ParsingState>) -> Result<ExifIter, ParsingError> {
    let (header, start) = match state {
        // header has been parsed, and header has been skipped, input data
        // is the IFD data
        Some(ParsingState::TiffHeader(header)) => (header, 0),
        None => {
            // header has not been parsed, input data includes IFD header
            let (_, header) = TiffHeader::parse(&input[..])?;
            let start = header.ifd0_offset as usize;
            if start > input.len() {
                return Err(ParsingError::ClearAndSkip(
                    start,
                    Some(ParsingState::TiffHeader(header)),
                ));
                // return Err(ParsingError::Need(start - data.len()));
            }

            (header, start)
        }
    };

    tracing::debug!(?header, offset = start);

    let data = &input[..];

    let mut ifd0 = match ImageFileDirectoryIter::try_new(
        0,
        input.make_associated(&data[start..]),
        header.ifd0_offset,
        header.endian,
        None,
    ) {
        Ok(ifd0) => ifd0,
        Err(e) => return Err(ParsingError::Failed(e.to_string())),
    };

    let tz = ifd0.find_tz_offset();
    ifd0.tz = tz.clone();
    let iter: ExifIter = ExifIter::new(input, header, tz, ifd0);

    tracing::debug!(?iter, "new ExifIter");

    Ok(iter)
}

#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use std::path::Path;

    use crate::{
        file::MimeImage,
        testkit::{open_sample, read_sample},
        values::URational,
    };
    use test_case::test_case;

    use super::*;

    #[test_case("exif.heic", "+43.29013+084.22713+1595.950CRSWGS_84/")]
    #[test_case("exif.jpg", "+22.53113+114.02148/")]
    fn gps(path: &str, gps_str: &str) {
        let f = open_sample(path).unwrap();
        let iter = parse_exif(f, None)
            .expect("should be Ok")
            .expect("should not be None");
        let gps_info = iter
            .parse_gps_info()
            .expect("should be parsed Ok")
            .expect("should not be None");

        // let gps_info = iter
        //     .consume_parse_gps_info()
        //     .expect("should be parsed Ok")
        //     .expect("should not be None");
        assert_eq!(gps_info.format_iso6709(), gps_str);
    }

    #[cfg(feature = "async")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    #[test_case("exif.heic", "+43.29013+084.22713+1595.950CRSWGS_84/")]
    #[test_case("exif.jpg", "+22.53113+114.02148/")]
    async fn gps_async(path: &str, gps_str: &str) {
        use tokio::fs::File;

        let f = File::open(Path::new("testdata").join(path)).await.unwrap();
        let iter = parse_exif_async(f, None)
            .await
            .expect("should be Ok")
            .expect("should not be None");

        let gps_str = gps_str.to_owned();
        let _ = tokio::spawn(async move {
            let exif: Exif = iter.into();
            let gps_info = exif.get_gps_info().expect("ok").expect("some");
            assert_eq!(gps_info.format_iso6709(), gps_str);
        })
        .await;
    }

    #[test_case(
        "exif.jpg",
        'N',
        [(22, 1), (31, 1), (5208, 100)].into(),
        'E',
        [(114, 1), (1, 1), (1733, 100)].into(),
        0u8,
        (0, 1).into(),
        None,
        None
    )]
    #[allow(clippy::too_many_arguments)]
    fn gps_info(
        path: &str,
        latitude_ref: char,
        latitude: LatLng,
        longitude_ref: char,
        longitude: LatLng,
        altitude_ref: u8,
        altitude: URational,
        speed_ref: Option<char>,
        speed: Option<URational>,
    ) {
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();

        let buf = read_sample(path).unwrap();
        let data = extract_exif_with_mime(MimeImage::Jpeg, &buf, None)
            .unwrap()
            .unwrap();

        let subslice_range = buf.subslice_range(data).unwrap();
        let iter = input_into_iter((buf, subslice_range), None).unwrap();
        let exif: Exif = iter.into();

        let gps = exif.get_gps_info().unwrap().unwrap();
        assert_eq!(
            gps,
            GPSInfo {
                latitude_ref,
                latitude,
                longitude_ref,
                longitude,
                altitude_ref,
                altitude,
                speed_ref,
                speed,
            }
        )
    }

    #[test_case("exif.heic")]
    fn tag_values(path: &str) {
        let f = open_sample(path).unwrap();
        let iter = parse_exif(f, None).unwrap().unwrap();
        let tags = [ExifTag::Make, ExifTag::Model];
        let res: Vec<String> = iter
            .clone()
            .filter(|e| e.tag().is_some_and(|t| tags.contains(&t)))
            .filter(|e| e.has_value())
            .map(|e| format!("{} => {}", e.tag().unwrap(), e.get_value().unwrap()))
            .collect();
        assert_eq!(res.join(", "), "Make => Apple, Model => iPhone 12 Pro");
    }
}
