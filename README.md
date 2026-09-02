# pbput

Put typed data directly onto the macOS pasteboard from files or stdin.

## Prerequisites

- macOS 13 or later.
- Rust 1.85 or later, only needed to build from source.

## Installation

### From a Release

Download the `pbput` binary for the current architecture from the
[releases page](https://github.com/audivir/pbput-rust/releases) and place it on the `PATH`.

### From Source

```bash
git clone https://github.com/audivir/pbput-rust
cd pbput-rust
cargo build --release
cp target/release/pbput ~/.local/bin/
```

## Usage

```bash
# copy a file, type is detected from its extension
pbput image.png

# copy from stdin with an explicit type
cat image.svg | pbput --type image/svg+xml

# copy from stdin using a UTI instead of a MIME type
cat image.svg | pbput --type public.svg-image

# copy from stdin without --type, content is sniffed instead
cat image.svg | pbput

# copy from stdin so apps like PowerPoint or ChemDraw can paste it too
cat image.svg | pbput -c --type image/svg+xml

# read a path from stdin and copy that file, as if it were the argument
echo "$(pwd)/image.svg" | pbput -p

# base64-encode the data before writing it to the pasteboard
cat image.png | pbput -b --type public.utf8-plain-text

# print the resolved type after copying
pbput -v image.png
```

### Options

| Flag | Description |
| ------------- | ---------------------------------------------------------------------------------------- |
| `-t`, `--type TYPE` | UTI or MIME type to declare on the pasteboard. If omitted, detected from the file extension, or guessed from stdin content otherwise. |
| `-c`, `--copy` | For stdin, also materialize the input as a temp file and put its file URL on the pasteboard, like Finder's Copy. |
| `-p`, `--path` | Treat the input as a path to a file, rather than data, and copy that file as if it were the argument. |
| `-b`, `--base64` | Base64-encode the data before writing it to the pasteboard. |
| `-v`, `--verbose` | Print the resolved type after copying. By default `pbput` produces no output on success. |
| `--version` | Print the version and exit. |

### Type Detection

Without `--type`, the type comes from, in order:

1. The OS-reported content type for a file, based on its extension.
1. For stdin (or a file with an unrecognized extension), the content itself: a few common
   text-format prefixes (SVG, XML, HTML, JSON), binary file signatures detected by
   [infer](https://docs.rs/infer), then `public.utf8-plain-text` for other valid UTF-8 data, or
   `public.data` as the final fallback.

### File-Aware Apps (PowerPoint, ChemDraw, etc.)

Some apps only accept pasted content by reading a real file, the same way Finder's Copy works,
rather than reading the raw pasteboard data directly. For a file argument, `pbput` always
writes a file URL representation alongside the raw content, so this works out of the box. For
stdin, pass `--copy` to have `pbput` write the input to a temp file first and reference that
instead.

## License

MIT License. See [LICENSE](LICENSE) for details.
