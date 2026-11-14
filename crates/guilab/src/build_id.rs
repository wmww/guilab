//! The running binary's GNU build id (from its loaded `PT_NOTE` segments), so the CLI can
//! tell whether the server runs the same build. Identical builds share it.

use std::ffi::{c_int, c_void};

unsafe extern "C" fn first_object(info: *mut libc::dl_phdr_info, _size: libc::size_t, out: *mut c_void) -> c_int {
    // SAFETY: called by dl_iterate_phdr with a valid info; `out` is our `String`.
    unsafe {
        let info = &*info;
        let out = &mut *(out as *mut String);
        let phdrs = std::slice::from_raw_parts(info.dlpi_phdr, info.dlpi_phnum as usize);
        for ph in phdrs.iter().filter(|p| p.p_type == libc::PT_NOTE) {
            let mut p = (info.dlpi_addr as usize + ph.p_vaddr as usize) as *const u8;
            let end = p.add(ph.p_memsz as usize);
            while p.add(12) <= end {
                let word = |i: usize| u32::from_ne_bytes(*(p.add(i * 4) as *const [u8; 4])) as usize;
                let (namesz, descsz, kind) = (word(0), word(1), word(2));
                let name = p.add(12);
                let desc = name.add(namesz.next_multiple_of(4));
                if desc.add(descsz) > end {
                    break;
                }
                // NT_GNU_BUILD_ID
                if kind == 3 && namesz == 4 && std::slice::from_raw_parts(name, 4) == b"GNU\0" {
                    *out = std::slice::from_raw_parts(desc, descsz).iter().map(|b| format!("{b:02x}")).collect();
                    return 1;
                }
                p = desc.add(descsz.next_multiple_of(4));
            }
        }
        // The first object is the executable itself: stop either way.
        1
    }
}

/// Hex build id, or "" if the binary has none.
pub fn build_id() -> String {
    let mut out = String::new();
    // SAFETY: the callback only reads the program headers it's given.
    unsafe { libc::dl_iterate_phdr(Some(first_object), &mut out as *mut String as *mut c_void) };
    out
}
