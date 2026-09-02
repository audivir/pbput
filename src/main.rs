use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use clap::Parser;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::NSPasteboard;
use objc2_foundation::{NSArray, NSData, NSString, NSURL};
use objc2_uniform_type_identifiers::{
    UTType, UTTypeData, UTTypeJSON, UTTypeSVG, UTTypeUTF8PlainText,
};
use uuid::Uuid;

/// Command-line arguments accepted by `pbput`.
#[derive(Parser, Debug)]
#[command(
    name = "pbput",
    version,
    about = "Copy FILE or stdin to the macOS pasteboard."
)]
struct Arguments {
    /// File to copy. Reads stdin if omitted or "-".
    path: Option<String>,

    /// UTI or MIME type, e.g. public.png, image/png.
    #[arg(short = 't', long = "type", value_name = "TYPE")]
    type_argument: Option<String>,

    /// Materialize stdin as a temp file for apps that paste by reading a file.
    #[arg(short = 'c', long = "copy")]
    copy_to_file: bool,

    /// Treat the input as a path to a file, rather than data.
    #[arg(short = 'p', long = "path")]
    input_is_path: bool,

    /// Base64-encode the data before writing it to the pasteboard.
    #[arg(short = 'b', long = "base64")]
    base64_encode: bool,

    /// Print the resolved type after copying.
    #[arg(short = 'v', long = "verbose")]
    verbose: bool,
}

/// Failure conditions reported by `pbput`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PbputError {
    UnsupportedType(String),
    Read(String),
    Pasteboard,
    TempFile,
}

impl fmt::Display for PbputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PbputError::UnsupportedType(type_argument) => {
                write!(f, "pbput: unsupported type: {type_argument}")
            }
            PbputError::Read(path) => write!(f, "pbput: cannot read {path}"),
            PbputError::Pasteboard => write!(f, "pbput: failed to write pasteboard"),
            PbputError::TempFile => write!(f, "pbput: failed to write temporary file for --copy"),
        }
    }
}

impl std::error::Error for PbputError {}

/// Resolves `value` as a UTI or MIME type, rejecting MIME strings the system cannot map to a
/// declared type.
fn resolve_type(value: &str) -> Option<Retained<UTType>> {
    let value = NSString::from_str(value);
    if let Some(uti) = UTType::typeWithIdentifier(&value) {
        return Some(uti);
    }
    let mime = UTType::typeWithMIMEType(&value)?;
    (!mime.isDynamic()).then_some(mime)
}

/// Resolves the pasteboard type for `data` read from `input`.
///
/// An explicit `type_argument` always wins. Otherwise, for a real file path (not `-`),
/// `detect_content_type` is tried before falling back to sniffing `data` itself.
fn determine_type(
    type_argument: Option<&str>,
    input: &str,
    data: &[u8],
    detect_content_type: impl FnOnce(&str) -> Option<Retained<UTType>>,
) -> Result<Retained<UTType>, PbputError> {
    if let Some(type_argument) = type_argument {
        return resolve_type(type_argument)
            .ok_or_else(|| PbputError::UnsupportedType(type_argument.to_string()));
    }
    if input != "-"
        && let Some(detected) = detect_content_type(input)
    {
        return Ok(detected);
    }
    Ok(fallback_type(data))
}

/// Guesses a pasteboard type for raw `data` with no extension or explicit type to go on.
///
/// Strips a leading UTF-8 BOM before sniffing, since neither our own checks nor `infer` (for
/// HTML) account for one, then tries SVG and JSON prefixes that `infer` cannot detect at all,
/// then binary and generic-text signatures via `infer` (which covers plain XML and HTML), then
/// `public.utf8-plain-text` for other valid UTF-8 data, or `public.data` as the final fallback.
/// The original `data`, BOM included, is what actually gets written to the pasteboard.
fn fallback_type(data: &[u8]) -> Retained<UTType> {
    let sniffed = data.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(data);
    if let Ok(text) = std::str::from_utf8(sniffed)
        && let Some(format) = sniff_text_format(text)
    {
        return format;
    }
    if let Some(signature) = sniff_binary_signature(sniffed) {
        return signature;
    }
    match std::str::from_utf8(data) {
        Ok(_) => unsafe { UTTypeUTF8PlainText }.into(),
        Err(_) => unsafe { UTTypeData }.into(),
    }
}

fn sniff_binary_signature(data: &[u8]) -> Option<Retained<UTType>> {
    let kind = infer::get(data)?;
    let mime = NSString::from_str(kind.mime_type());
    let uti = UTType::typeWithMIMEType(&mime)?;
    (!uti.isDynamic()).then_some(uti)
}

/// `infer` has no SVG or JSON matchers, so those are sniffed by hand. SVG detection just checks
/// for `<svg` anywhere in the document, so it can misfire on XML that merely mentions `<svg` (e.g.
/// in a comment); that false positive is accepted rather than parsing the document properly.
fn sniff_text_format(text: &str) -> Option<Retained<UTType>> {
    let trimmed = text.trim();
    if (trimmed.starts_with("<?xml") || trimmed.starts_with("<svg")) && trimmed.contains("<svg") {
        return Some(unsafe { UTTypeSVG }.into());
    }
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return Some(unsafe { UTTypeJSON }.into());
    }
    None
}

/// Builds a unique file path for `r#type` inside `directory`, named so a receiving app can infer
/// its format from the extension alone.
fn temporary_file_url(r#type: &UTType, directory: &Path) -> PathBuf {
    let name = format!("pbput-{}", Uuid::new_v4());
    let path = directory.join(name);
    match r#type.preferredFilenameExtension() {
        Some(extension) => path.with_extension(extension.to_string()),
        None => path,
    }
}

fn die(error: PbputError) -> ! {
    eprintln!("{error}");
    std::process::exit(2);
}

/// Resolves the OS-reported content type for a file at `path`, based on its extension.
fn detect_content_type(path: &str) -> Option<Retained<UTType>> {
    let extension = Path::new(path).extension()?.to_str()?;
    let uti = UTType::typeWithFilenameExtension(&NSString::from_str(extension))?;
    (!uti.isDynamic()).then_some(uti)
}

fn write_file_url(pasteboard: &NSPasteboard, path: &Path) -> bool {
    let Some(url) = NSURL::from_file_path(path) else {
        return false;
    };
    let writer = ProtocolObject::from_retained(url);
    pasteboard.writeObjects(&NSArray::from_retained_slice(&[writer]))
}

/// Removes `pbput-*` temp files older than `MAX_AGE` from `directory`.
///
/// Files written for `--copy` must outlive this process so a receiving app can read them later,
/// so they cannot be deleted right after writing; this bounds their accumulation instead.
fn cleanup_stale_temp_files(directory: &Path) {
    const MAX_AGE: std::time::Duration = std::time::Duration::from_secs(60 * 60);
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let is_stale = entry
            .file_name()
            .to_str()
            .is_some_and(|n| n.starts_with("pbput-"))
            && entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .is_ok_and(|modified| modified.elapsed().is_ok_and(|age| age > MAX_AGE));
        if is_stale {
            fs::remove_file(entry.path()).ok();
        }
    }
}

fn main() {
    let arguments = Arguments::parse();
    let mut input = arguments.path.clone().unwrap_or_else(|| "-".to_string());

    let mut data = if input == "-" {
        let mut buffer = Vec::new();
        std::io::stdin()
            .read_to_end(&mut buffer)
            .unwrap_or_else(|_| die(PbputError::Read(input.clone())));
        buffer
    } else {
        fs::read(&input).unwrap_or_else(|_| die(PbputError::Read(input.clone())))
    };

    if arguments.input_is_path {
        input = String::from_utf8(data)
            .unwrap_or_else(|_| die(PbputError::Read(input.clone())))
            .trim()
            .to_string();
        data = fs::read(&input).unwrap_or_else(|_| die(PbputError::Read(input.clone())));
    }

    if arguments.base64_encode {
        data = BASE64.encode(&data).into_bytes();
    }

    let resolved_type = determine_type(
        arguments.type_argument.as_deref(),
        &input,
        &data,
        detect_content_type,
    )
    .unwrap_or_else(|error| die(error));

    let temp_path = if input == "-" && arguments.copy_to_file {
        let temp_dir = std::env::temp_dir();
        cleanup_stale_temp_files(&temp_dir);
        let path = temporary_file_url(&resolved_type, &temp_dir);
        if fs::write(&path, &data).is_err() {
            die(PbputError::TempFile);
        }
        Some(path)
    } else {
        None
    };

    let pasteboard = NSPasteboard::generalPasteboard();
    pasteboard.clearContents();

    if input != "-" {
        if !write_file_url(&pasteboard, Path::new(&input)) {
            die(PbputError::Pasteboard);
        }
    } else if let Some(temp_path) = &temp_path
        && !write_file_url(&pasteboard, temp_path)
    {
        die(PbputError::Pasteboard);
    }

    let identifier = resolved_type.identifier();
    let type_list = NSArray::from_slice(&[&*identifier]);
    unsafe { pasteboard.addTypes_owner(&type_list, None) };

    if !pasteboard.setData_forType(Some(&NSData::with_bytes(&data)), &identifier) {
        die(PbputError::Pasteboard);
    }

    if arguments.verbose {
        match resolved_type.preferredMIMEType() {
            Some(mime) => println!("{identifier} ({mime})"),
            None => println!("{identifier}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_uniform_type_identifiers::UTTypePNG;

    fn id(t: &UTType) -> String {
        t.identifier().to_string()
    }

    #[test]
    fn resolves_uti() {
        assert_eq!(id(&resolve_type("public.png").unwrap()), "public.png");
    }

    #[test]
    fn resolves_mime_type() {
        assert_eq!(id(&resolve_type("image/png").unwrap()), "public.png");
    }

    #[test]
    fn rejects_garbage() {
        assert!(resolve_type("not-a-real-type").is_none());
    }

    #[test]
    fn rejects_mime_shaped_garbage() {
        // UTType(mimeType:) synthesizes a dynamic placeholder for unknown MIME shapes instead
        // of returning nil, so resolve_type must reject those explicitly.
        assert!(resolve_type("application/x-pbput-test-garbage").is_none());
    }

    #[test]
    fn determine_type_prefers_explicit_argument() {
        let resolved =
            determine_type(Some("public.png"), "photo.jpg", b"", |_| unreachable!()).unwrap();
        assert_eq!(id(&resolved), "public.png");
    }

    #[test]
    fn determine_type_rejects_invalid_explicit_argument() {
        let error = determine_type(Some("bogus"), "-", b"", |_| unreachable!()).unwrap_err();
        assert_eq!(error, PbputError::UnsupportedType("bogus".to_string()));
    }

    #[test]
    fn determine_type_uses_detected_content_type_for_file() {
        let resolved =
            determine_type(None, "photo.png", b"", |_| resolve_type("public.png")).unwrap();
        assert_eq!(id(&resolved), "public.png");
    }

    #[test]
    fn determine_type_falls_back_for_stdin_without_explicit_type() {
        let resolved = determine_type(None, "-", b"hello", |_| unreachable!()).unwrap();
        assert_eq!(id(&resolved), "public.utf8-plain-text");
    }

    #[test]
    fn determine_type_falls_back_for_undetectable_file() {
        let resolved = determine_type(None, "photo.xyz", b"hello", |_| None).unwrap();
        assert_eq!(id(&resolved), "public.utf8-plain-text");
    }

    #[test]
    fn fallback_type_is_utf8_plain_text_for_empty_data() {
        assert_eq!(id(&fallback_type(b"")), "public.utf8-plain-text");
    }

    #[test]
    fn fallback_type_is_data_for_non_utf8_bytes() {
        assert_eq!(id(&fallback_type(&[0xFF, 0xFE, 0x00])), "public.data");
    }

    #[test]
    fn fallback_type_recognizes_binary_signatures() {
        let cases: &[(&[u8], &str)] = &[
            (
                &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0],
                "public.png",
            ),
            (&[0xFF, 0xD8, 0xFF, 0, 0, 0], "public.jpeg"),
            (b"%PDF-1.4 rest of the file", "com.adobe.pdf"),
            (&[0x50, 0x4B, 0x03, 0x04, 0, 0], "public.zip-archive"),
        ];
        for (data, expected) in cases {
            assert_eq!(id(&fallback_type(data)), *expected, "data {data:?}");
        }
    }

    #[test]
    fn fallback_type_recognizes_text_formats() {
        let cases: &[(&str, &str)] = &[
            (
                r#"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg"></svg>"#,
                "public.svg-image",
            ),
            ("<svg></svg>", "public.svg-image"),
            (r#"<?xml version="1.0"?><root></root>"#, "public.xml"),
            ("<!DOCTYPE html><html></html>", "public.html"),
            ("<html><body></body></html>", "public.html"),
            (r#"{"key": "value"}"#, "public.json"),
            ("[1, 2, 3]", "public.json"),
        ];
        for (text, expected) in cases {
            assert_eq!(
                id(&fallback_type(text.as_bytes())),
                *expected,
                "text {text:?}"
            );
        }
    }

    #[test]
    fn fallback_type_ignores_leading_whitespace_when_sniffing_text() {
        assert_eq!(id(&fallback_type(b"  \n<svg></svg>")), "public.svg-image");
    }

    #[test]
    fn temporary_file_url_uses_type_extension() {
        let directory = Path::new("/tmp");
        let url = temporary_file_url(unsafe { UTTypePNG }, directory);

        assert_eq!(url.parent().unwrap(), directory);
        assert_eq!(url.extension().unwrap(), "png");
    }

    #[test]
    fn temporary_file_url_omits_extension_when_type_has_none() {
        let directory = Path::new("/tmp");
        let url = temporary_file_url(unsafe { UTTypeData }, directory);

        assert!(url.extension().is_none());
    }

    #[test]
    fn temporary_file_url_is_unique() {
        let directory = Path::new("/tmp");
        assert_ne!(
            temporary_file_url(unsafe { UTTypePNG }, directory),
            temporary_file_url(unsafe { UTTypePNG }, directory)
        );
    }
}
