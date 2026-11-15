//! Streaming ZIP records: stored members, data descriptors and ZIP64 where sizes or offsets need it.
//! Format reference: PKWARE APPNOTE 6.3.10, sections 4.3 and 4.5.3.

const LOCAL_HEADER: u32 = 0x0403_4b50;
const DATA_DESCRIPTOR: u32 = 0x0807_4b50;
const CENTRAL_HEADER: u32 = 0x0201_4b50;
const ZIP64_END: u32 = 0x0606_4b50;
const ZIP64_LOCATOR: u32 = 0x0706_4b50;
const END: u32 = 0x0605_4b50;
const ZIP64_EXTRA: u16 = 0x0001;
const VERSION_DEFAULT: u16 = 20;
const VERSION_ZIP64: u16 = 45;
const MADE_BY_UNIX: u16 = 3 << 8;
const FLAG_DATA_DESCRIPTOR: u16 = 1 << 3;
const FLAG_UTF8: u16 = 1 << 11;
const STORED: u16 = 0;
const U32_LIMIT: u64 = u32::MAX as u64;
const DIRECTORY_ATTRIBUTES: u32 = (0o040_755 << 16) | 0x10;
const FILE_ATTRIBUTES: u32 = 0o100_644 << 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberKind {
    Directory,
    File,
}

/// One archive member. Directory names end in `/`; files carry their exact length.
#[derive(Debug, Clone)]
pub struct Member {
    pub name: String,
    pub kind: MemberKind,
    pub size: u64,
    pub dos_time: u16,
    pub dos_date: u16,
}

impl Member {
    // 0xFFFFFFFF is the ZIP64 sentinel, so it needs ZIP64 too.
    fn zip64_size(&self) -> bool {
        self.size >= U32_LIMIT
    }
}

fn put16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}
fn put32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}
fn put64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn name_length(member: &Member) -> u16 {
    u16::try_from(member.name.len()).expect("archive paths are bounded below 64 KiB")
}

fn flags(member: &Member) -> u16 {
    match member.kind {
        MemberKind::File => FLAG_DATA_DESCRIPTOR | FLAG_UTF8,
        MemberKind::Directory => FLAG_UTF8,
    }
}

/// Local header. File CRC and sizes follow in the data descriptor.
pub fn local_header(member: &Member) -> Vec<u8> {
    let zip64 = member.zip64_size();
    let mut out = Vec::with_capacity(30 + member.name.len() + 20);
    put32(&mut out, LOCAL_HEADER);
    put16(
        &mut out,
        if zip64 {
            VERSION_ZIP64
        } else {
            VERSION_DEFAULT
        },
    );
    put16(&mut out, flags(member));
    put16(&mut out, STORED);
    put16(&mut out, member.dos_time);
    put16(&mut out, member.dos_date);
    put32(&mut out, 0);
    let placeholder = if zip64 { u32::MAX } else { 0 };
    put32(&mut out, placeholder);
    put32(&mut out, placeholder);
    put16(&mut out, name_length(member));
    put16(&mut out, if zip64 { 20 } else { 0 });
    out.extend_from_slice(member.name.as_bytes());
    if zip64 {
        // Readers take 8-byte descriptor sizes from this field's presence.
        put16(&mut out, ZIP64_EXTRA);
        put16(&mut out, 16);
        put64(&mut out, 0);
        put64(&mut out, 0);
    }
    out
}

/// Data descriptor written after a file's bytes.
pub fn data_descriptor(member: &Member, crc32: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(24);
    put32(&mut out, DATA_DESCRIPTOR);
    put32(&mut out, crc32);
    if member.zip64_size() {
        put64(&mut out, member.size);
        put64(&mut out, member.size);
    } else {
        put32(&mut out, member.size as u32);
        put32(&mut out, member.size as u32);
    }
    out
}

/// Central directory header for a member whose local header starts at `offset`.
pub fn central_header(member: &Member, crc32: u32, offset: u64) -> Vec<u8> {
    let zip64_size = member.zip64_size();
    let zip64_offset = offset >= U32_LIMIT;
    let mut extra = Vec::new();
    if zip64_size {
        put64(&mut extra, member.size);
        put64(&mut extra, member.size);
    }
    if zip64_offset {
        put64(&mut extra, offset);
    }
    let zip64 = zip64_size || zip64_offset;
    let mut out = Vec::with_capacity(46 + member.name.len() + 4 + extra.len());
    put32(&mut out, CENTRAL_HEADER);
    put16(&mut out, MADE_BY_UNIX | VERSION_ZIP64);
    put16(
        &mut out,
        if zip64 {
            VERSION_ZIP64
        } else {
            VERSION_DEFAULT
        },
    );
    put16(&mut out, flags(member));
    put16(&mut out, STORED);
    put16(&mut out, member.dos_time);
    put16(&mut out, member.dos_date);
    put32(&mut out, crc32);
    let size = if zip64_size {
        u32::MAX
    } else {
        member.size as u32
    };
    put32(&mut out, size);
    put32(&mut out, size);
    put16(&mut out, name_length(member));
    put16(&mut out, if zip64 { 4 + extra.len() as u16 } else { 0 });
    put16(&mut out, 0);
    put16(&mut out, 0);
    put16(&mut out, 0);
    put32(
        &mut out,
        match member.kind {
            MemberKind::Directory => DIRECTORY_ATTRIBUTES,
            MemberKind::File => FILE_ATTRIBUTES,
        },
    );
    put32(
        &mut out,
        if zip64_offset {
            u32::MAX
        } else {
            offset as u32
        },
    );
    out.extend_from_slice(member.name.as_bytes());
    if zip64 {
        put16(&mut out, ZIP64_EXTRA);
        put16(&mut out, extra.len() as u16);
        out.extend_from_slice(&extra);
    }
    out
}

/// End records for a central directory of `size` bytes starting at `offset`.
pub fn end_records(entries: u64, offset: u64, size: u64) -> Vec<u8> {
    let zip64 = entries >= u16::MAX as u64 || offset >= U32_LIMIT || size >= U32_LIMIT;
    let mut out = Vec::with_capacity(56 + 20 + 22);
    if zip64 {
        let zip64_end = offset + size;
        put32(&mut out, ZIP64_END);
        put64(&mut out, 44);
        put16(&mut out, MADE_BY_UNIX | VERSION_ZIP64);
        put16(&mut out, VERSION_ZIP64);
        put32(&mut out, 0);
        put32(&mut out, 0);
        put64(&mut out, entries);
        put64(&mut out, entries);
        put64(&mut out, size);
        put64(&mut out, offset);
        put32(&mut out, ZIP64_LOCATOR);
        put32(&mut out, 0);
        put64(&mut out, zip64_end);
        put32(&mut out, 1);
    }
    put32(&mut out, END);
    put16(&mut out, 0);
    put16(&mut out, 0);
    let count = if zip64 { u16::MAX } else { entries as u16 };
    put16(&mut out, count);
    put16(&mut out, count);
    put32(&mut out, if zip64 { u32::MAX } else { size as u32 });
    put32(&mut out, if zip64 { u32::MAX } else { offset as u32 });
    put16(&mut out, 0);
    out
}

/// MS-DOS (time, date) for Unix seconds in UTC, clamped to the representable 1980–2107 range.
pub fn dos_timestamp(unix_seconds: i64) -> (u16, u16) {
    const MIN: i64 = 315_532_800; // 1980-01-01T00:00:00Z
    const MAX: i64 = 4_354_819_198; // 2107-12-31T23:59:58Z
    let seconds = unix_seconds.clamp(MIN, MAX);
    let days = seconds.div_euclid(86_400);
    let of_day = seconds.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let time = ((of_day / 3600) << 11) | (((of_day % 3600) / 60) << 5) | ((of_day % 60) / 2);
    let date = ((year - 1980) << 9) | (month << 5) | day;
    (time as u16, date as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(kind: MemberKind, size: u64) -> Member {
        Member {
            name: "a/b.bin".into(),
            kind,
            size,
            dos_time: 0,
            dos_date: 0x21,
        }
    }

    fn u16_at(bytes: &[u8], at: usize) -> u16 {
        u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap())
    }
    fn u32_at(bytes: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
    }
    fn u64_at(bytes: &[u8], at: usize) -> u64 {
        u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
    }

    #[test]
    fn small_members_use_classic_fields() {
        let file = member(MemberKind::File, 1234);
        let local = local_header(&file);
        assert_eq!(u16_at(&local, 4), VERSION_DEFAULT);
        assert_eq!(u16_at(&local, 28), 0, "no extra field");
        let descriptor = data_descriptor(&file, 0xdead_beef);
        assert_eq!(descriptor.len(), 16);
        assert_eq!(u32_at(&descriptor, 8), 1234);
        let central = central_header(&file, 0xdead_beef, 99);
        assert_eq!(u32_at(&central, 16), 0xdead_beef);
        assert_eq!(u32_at(&central, 20), 1234);
        assert_eq!(u32_at(&central, 42), 99);
        assert_eq!(u16_at(&central, 30), 0);
        assert_eq!(end_records(1, 10, 20).len(), 22);
    }

    #[test]
    fn members_and_offsets_beyond_four_gib_use_zip64_fields() {
        let size = 5 * 1024 * 1024 * 1024;
        let offset = 6 * 1024 * 1024 * 1024;
        let file = member(MemberKind::File, size);
        let local = local_header(&file);
        assert_eq!(u16_at(&local, 4), VERSION_ZIP64);
        assert_eq!(u32_at(&local, 18), u32::MAX);
        assert_eq!(u16_at(&local, 28), 20);
        assert_eq!(u16_at(&local, 30 + file.name.len()), ZIP64_EXTRA);
        let descriptor = data_descriptor(&file, 7);
        assert_eq!(descriptor.len(), 24);
        assert_eq!(u64_at(&descriptor, 8), size);
        assert_eq!(u64_at(&descriptor, 16), size);

        let central = central_header(&file, 7, offset);
        assert_eq!(u32_at(&central, 20), u32::MAX);
        assert_eq!(u32_at(&central, 24), u32::MAX);
        assert_eq!(u32_at(&central, 42), u32::MAX);
        let extra = 46 + file.name.len();
        assert_eq!(u16_at(&central, extra), ZIP64_EXTRA);
        assert_eq!(u16_at(&central, extra + 2), 24);
        assert_eq!(u64_at(&central, extra + 4), size);
        assert_eq!(u64_at(&central, extra + 12), size);
        assert_eq!(u64_at(&central, extra + 20), offset);

        // Only the offset overflows: the extra field holds just the offset.
        let small = member(MemberKind::File, 10);
        let central = central_header(&small, 7, offset);
        assert_eq!(u32_at(&central, 20), 10);
        assert_eq!(u16_at(&central, extra + 2), 8);
        assert_eq!(u64_at(&central, extra + 4), offset);

        let end = end_records(3, offset, 300);
        assert_eq!(u32_at(&end, 0), ZIP64_END);
        assert_eq!(u64_at(&end, 48), offset);
        assert_eq!(u32_at(&end, 56), ZIP64_LOCATOR);
        assert_eq!(u64_at(&end, 64), offset + 300);
        assert_eq!(u32_at(&end, 76), END);
        assert_eq!(u32_at(&end, 76 + 16), u32::MAX);
    }

    #[test]
    fn exact_sentinel_size_uses_zip64() {
        assert!(member(MemberKind::File, u32::MAX as u64).zip64_size());
        assert!(!member(MemberKind::File, u32::MAX as u64 - 1).zip64_size());
    }

    #[test]
    fn directories_have_no_descriptor_flag_and_directory_attributes() {
        let directory = Member {
            name: "a/".into(),
            ..member(MemberKind::Directory, 0)
        };
        let local = local_header(&directory);
        assert_eq!(u16_at(&local, 6), FLAG_UTF8);
        let central = central_header(&directory, 0, 0);
        assert_eq!(u32_at(&central, 38), DIRECTORY_ATTRIBUTES);
    }

    #[test]
    fn dos_timestamps_convert_utc_and_clamp() {
        // 2024-02-29T13:45:58Z
        let (time, date) = dos_timestamp(1_709_214_358);
        assert_eq!(date, ((2024 - 1980) << 9) | (2 << 5) | 29);
        assert_eq!(time, (13 << 11) | (45 << 5) | 29);
        assert_eq!(dos_timestamp(0), (0, (1 << 5) | 1));
        assert_eq!(dos_timestamp(i64::MAX).1 >> 9, 127);
    }
}
