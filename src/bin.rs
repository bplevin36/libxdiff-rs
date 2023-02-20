
use libxdiff::MMFile;

pub fn main() {
    let mut base = MMFile::from_bytes(b"header\nhello world\n");
    let mut f1 = MMFile::from_bytes(b"header\nHello World\n");
    let mut f2 = MMFile::from_bytes(b"header\nhello world!\n");

    MMFile::merge(&mut base, &mut f1, &mut f2);

}
