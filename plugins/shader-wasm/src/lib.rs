//! Minimal no_std Wasm plugin for the `shader` Notist element.
//!
//! ABI (not final WIT): host writes a binary request into Wasm memory and
//! calls `evaluate(ptr, len)`. The module writes a JSON response into a static
//! buffer and returns its pointer.
//!
//! Request layout:
//! ```text
//! 0: u32 source_len
//! 4: source bytes
//! after source: i32 width
//! after source + 4: i32 height
//! ```

#![no_std]

use core::fmt::Write as _;
use core::panic::PanicInfo;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}

static mut RESPONSE: [u8; 65536] = [0; 65536];

struct BufWriter {
    buf: &'static mut [u8],
    pos: usize,
}

impl BufWriter {
    fn new() -> Self {
        // Safety: single-threaded wasm, only used inside evaluate.
        let buf = unsafe { &mut *core::ptr::addr_of_mut!(RESPONSE) };
        buf.fill(0);
        Self { buf, pos: 0 }
    }
}

impl core::fmt::Write for BufWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let bytes = s.as_bytes();
        if self.pos + bytes.len() > self.buf.len() {
            return Err(core::fmt::Error);
        }
        self.buf[self.pos..self.pos + bytes.len()].copy_from_slice(bytes);
        self.pos += bytes.len();
        Ok(())
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn evaluate(ptr: *const u8, len: usize) -> *const u8 {
    // Safety: host must provide a valid buffer of at least len bytes.
    let request = unsafe { core::slice::from_raw_parts(ptr, len) };
    let mut writer = BufWriter::new();

    if request.len() < 8 {
        let _ = writer.write_str(r#"{"ok":false,"error":"request too short"}"#);
        return writer.buf.as_ptr();
    }

    let source_len = u32::from_le_bytes([request[0], request[1], request[2], request[3]]) as usize;
    if 4 + source_len + 8 > request.len() {
        let _ = writer.write_str(r#"{"ok":false,"error":"request truncated"}"#);
        return writer.buf.as_ptr();
    }

    let source_bytes = &request[4..4 + source_len];
    let source = core::str::from_utf8(source_bytes).unwrap_or("");
    let width_offset = 4 + source_len;
    let height_offset = width_offset + 4;
    let width = i32::from_le_bytes([
        request[width_offset],
        request[width_offset + 1],
        request[width_offset + 2],
        request[width_offset + 3],
    ]);
    let height = i32::from_le_bytes([
        request[height_offset],
        request[height_offset + 1],
        request[height_offset + 2],
        request[height_offset + 3],
    ]);

    let _ = write!(
        writer,
        r#"{{"ok":true,"fields":{{"source":""#
    );
    // Simple JSON string escaping (only quotes and backslashes).
    for &b in source_bytes {
        match b {
            b'"' => {
                let _ = writer.write_str("\\\"");
            }
            b'\\' => {
                let _ = writer.write_str("\\\\");
            }
            _ => {
                let _ = writer.write_str(core::str::from_utf8(core::slice::from_ref(&b)).unwrap_or(""));
            }
        }
    }
    let _ = write!(
        writer,
        r#"","width":{},"height":{},"wgpu":false}},"warning":"wasm plugin (wgpu rendering happens in the HTML WebGPU target)"}}"#,
        width, height
    );

    writer.buf.as_ptr()
}
