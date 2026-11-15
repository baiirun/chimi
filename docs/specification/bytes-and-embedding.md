# Bytes, Byte-Level Integers, and Build-Time Embedding

## Status

This is the normative contract for binary data (G6), fixed-width integers
inside binary data (L5), and files included at build time (G19), from the
[language completion plan](../plans/language-completion-plan.md). It is part
of Phase 6a (CHI-209). `std/bytes` and byte-level integers are implemented on
both backends (CHI-210). [Embedding](#build-time-embedding-g19) is specified
here and not implemented yet (CHI-211).

The words MUST, MUST NOT, SHOULD, SHOULD NOT, and MAY are normative. The
module follows the [standard library conventions](standard-library-conventions.md).

## Goals

- One immutable binary type that files, sockets, pipes, hashing, and
  encodings share, so byte streams (G18) and `std/fs`, `std/exec`, and
  `std/net` all speak `Bytes`.
- A defined, strict conversion between `Text` and `Bytes`, with lossy
  decoding visible in its name.
- Reading and writing the integers binary protocols use, without adding
  sized integer types to the language.
- Including a file's contents in a program at build time, with no new syntax.

## Example

<!-- chimi: check -->
```chimi
import std/bytes as Bytes
import std/bytes.{Bytes, IntLayout}

/// One frame: a 16-bit big-endian length, then that many bytes of payload.
fn frame(payload: Bytes) -> Bytes {
  Bytes.concat([Bytes.from_int(Bytes.length(payload), IntLayout.U16Be), payload])
}

/// The first frame in `buffer` and the bytes after it, or `None` when the
/// buffer does not hold a whole frame yet.
fn next_frame(buffer: Bytes) -> Option<(Bytes, Bytes)> {
  case Bytes.read_int(buffer, 0, IntLayout.U16Be) {
    None -> None
    Some(size) -> case Bytes.length(buffer) >= 2 + size {
      True -> Some((Bytes.slice(buffer, 2..2 + size), Bytes.slice(buffer, 2 + size..Bytes.length(buffer))))
      False -> None
    }
  }
}
```

## `std/bytes` (G6)

### The type

`pub native type Bytes` is a finite sequence of bytes. Each byte is observed
as an `Int` in `0..=255`. Its registry properties:

| Property | Value |
| --- | --- |
| Equatable | always: two values are equal when they hold the same bytes |
| Ordered | always: lexicographic by unsigned byte, a shorter prefix first, as `Text` orders by UTF-8 bytes |
| Concurrent property | shareable |
| Constant-representable | yes |
| Render override | `Bytes(68 69)`: two lowercase hex digits per byte, separated by spaces; `Bytes()` when empty |

Rendering as hex rather than as text or decimal numbers keeps binary data
readable and makes no claim that the bytes are text. The outcome record
writes a `Bytes` as `{"bytes":"6869"}`.

### Surface

| Function | Contract |
| --- | --- |
| `empty() -> Bytes` | no bytes |
| `from(values: List<Int>) -> Bytes` | the listed bytes; defects when an element is outside `0..=255`, naming its index |
| `length(bytes) -> Int` | the number of bytes, O(1) |
| `is_empty(bytes) -> Bool` | `length(bytes) == 0` |
| `get(bytes, index) -> Option<Int>` | the byte at `index`, or `None` outside `0..length` |
| `index_of(bytes, needle) -> Option<Int>` | the first offset of `needle`; defects on an empty needle, as `Text.contains` does |
| `contains(bytes, needle) -> Bool` | whether `needle` occurs; defects on an empty needle |
| `starts_with(bytes, prefix) -> Bool`, `ends_with(bytes, suffix) -> Bool` | an empty prefix or suffix always matches |
| `read_int(bytes, at, layout) -> Option<Int>` | see [Byte-level integers](#byte-level-integers-l5) |
| `append(bytes, other) -> Bytes` | `bytes` then `other` |
| `concat(parts: List<Bytes>) -> Bytes` | the parts joined in order, in one copy |
| `slice(bytes, range) -> Bytes` | the bytes in `range` clamped to `0..length`, so it never fails, as `List.slice` does |
| `split_once(bytes, separator) -> Option<(Bytes, Bytes)>` | the bytes before and after the first `separator`; defects on an empty separator |
| `to_list(bytes) -> List<Int>` | the bytes as `Int`s |
| `from_text(text) -> Bytes` | the UTF-8 encoding of `text`; total |
| `to_text(bytes) -> Text errors DecodeError` | the text the bytes encode as UTF-8 |
| `to_text_lossy(bytes) -> Text` | the same, with each invalid sequence replaced by U+FFFD |
| `from_int(value, layout) -> Bytes` | see [Byte-level integers](#byte-level-integers-l5) |

`pub enum DecodeError { InvalidUtf8(at: Int) }` is the one failure. `at` is
the offset where the first invalid sequence starts: every byte before it
decodes. The payload carries a position, not a copy of the input, as the
conventions require of error payloads.

Rules:

- Positions and lengths count bytes. `Bytes` has a bare `length`, unlike
  `Text`, because a byte sequence has only one unit.
- `to_text` accepts exactly the UTF-8 that Unicode Table 3-7 allows: no
  overlong forms, no surrogates (U+D800 to U+DFFF), nothing past U+10FFFF.
  A zero byte is valid UTF-8 and decodes to U+0000.
- `to_text_lossy` replaces each *maximal subpart* of an ill-formed sequence
  with one U+FFFD, as the Unicode Standard (section 3.9, "U+FFFD
  Substitution of Maximal Subparts") recommends and as Rust's
  `String::from_utf8_lossy` and the WHATWG decoder do. So `E2 82` (a cut-off
  `€`) becomes one U+FFFD, and `E0 80 80` (overlong) becomes three.
- `Bytes.to_text(Bytes.from_text(text))` returns `text` for every `text`.
- The conversion lives in `std/bytes`, which depends on `Text`; `std/text`
  knows nothing of `Bytes`, as the conventions' conversion rule requires.
- Every function is `pure`, so constants MAY build `Bytes`.

### Why this surface

- Search returns a position (`index_of`) because binary framing needs one,
  and `split_once` covers the common position-free case, as the conventions
  ask modules to offer first.
- There is no `Bytes.set` or builder. Values are immutable, and building a
  message is one `concat` over its parts, which is linear. Appending one part
  at a time copies the growing prefix each time; G18 streams and a future
  persistent byte buffer can address bulk building when a program shows the
  need.
- Hex and base64 are not here. They belong in `std/hex` and `std/base64`
  (standard library map), written in Chimi over this surface.

## Byte-level integers (L5)

`Int` stays the only integer type. A layout names how an integer sits in
bytes:

<!-- chimi: fragment -->
```chimi
pub enum IntLayout {
  U8
  I8
  U16Be
  U16Le
  I16Be
  I16Le
  U32Be
  U32Le
  I32Be
  I32Le
  I64Be
  I64Le
}
```

`U` is unsigned and `I` is two's-complement signed; the number is the width
in bits; `Be` is big-endian (most significant byte first, network order) and
`Le` is little-endian.

- `Bytes.read_int(bytes, at, layout) -> Option<Int>` reads the integer that
  starts at offset `at`. It returns `None` when `at` is negative or the field
  would run past the end, so parsing untrusted input needs no separate bounds
  check. A signed layout sign-extends.
- `Bytes.from_int(value, layout) -> Bytes` writes `value` in exactly the
  layout's width. It defects when `value` is outside the layout's range, for
  example `256` for `U8` or `-1` for any unsigned layout, with a message that
  names the value and the layout. Encoding a value that does not fit is a
  broken precondition, like `Int` overflow.
- There is no `U64`: an `Int` holds 64 bits signed. A 64-bit unsigned field,
  such as a hash, is read with `I64Be` or `I64Le`, which gives the same bits.

One function over a layout enum, rather than a family such as `read_u16_be`
and `read_u32_le`, follows "one uniform rule over special cases": the twelve
layouts are one operation with one contract, and the enum is exhaustive in
`case`. The layout comes last, so `Bytes.read_int(header, 0, IntLayout.U16Be)`
reads in the order a protocol description lists a field.

## Build-time embedding (G19)

<!-- chimi: pending CHI-211 -->
```chimi
import std/embed as Embed

const index_page = Embed.text("assets/index.html")
const logo = Embed.bytes("assets/logo.png")
```

- `std/embed` declares `text(path: Text) -> Text` and `bytes(path: Text) ->
  Bytes`. Each reads a file when the program is built, and the constant holds
  its contents, so the executable needs no file at run time.
- A call is valid only as part of a module constant's initializer, because
  only constants are evaluated at build time (`E5206` elsewhere). The path
  MUST be a text literal, so the build knows every input before evaluation.
- The path is relative to the directory of the module's source file, uses
  `/` as the separator, and MUST NOT be absolute or contain an empty, `.`, or
  `..` segment (`E5207`), so a module embeds only files beside it or below
  it, as Go's `//go:embed` requires.
- A missing or unreadable file is a build error (`E5208`), as is a file that
  `Embed.text` reads and that is not valid UTF-8 (`E5209`, which names the
  offset of the first invalid byte).
- The embedded value counts toward the constant's 16 MiB stored-size budget
  (`E5203`). A `Bytes` constant is stored as hex text, so a file of up to
  8 MiB fits.
- The constant cache keys on the embedded files' contents as well as the
  constant's Core, so editing an embedded file rebuilds the constant.

This needs no new syntax and no macro: it reuses the constant rules, which
already make build-time evaluation deterministic and side-effect free. Macros
and pluggability remain an open question for Byron.

## Implementation notes

- The interpreter holds a `Bytes` as `RuntimeValue::Bytes(Arc<[u8]>)`; the
  native runtime as `VALUE_BYTES`, sharing Text's `as.text` buffer so the
  collector, heap copies, and transfers treat it like a Text's buffer.
- Program data stores a `Bytes` constant as row kind `CHIMI_CONSTANT_BYTES`
  with the id of its lowercase hex text in the string table.
- `to_text` is Chimi over two private natives: `valid_utf8_length`, and
  `to_text_lossy`, which is exact when the whole input is valid.
  `read_int` and `from_int` pass the layout to the natives `read_int_code`
  and `int_bytes` as its declaration index, as `std/decimal` passes a
  `Rounding`.
- The std-tests programs under `compiler/chimi/std-tests/bytes/` cover every
  function, every layout, the UTF-8 edge cases above, the defects, constants,
  and Map keys and Set items, on both backends; `tests/language.rs` covers a
  `Bytes` crossing task heaps natively.
