#[macro_use]
extern crate log;
use io_partition::Partition;
use std::fmt;
use std::io;
use std::io::{Read, Seek, SeekFrom};

fn get_bit(byte: u8, id: usize) -> Option<bool> {
    if id < 8 {
        Some((byte >> (7 - id) << 7) >= 1)
    } else {
        None
    }
}

#[derive(Debug)]
pub enum PXError {
    IOError(io::Error),
    InvalidHeaderMagic([u8; 5]),
    InvalidDecompressedLength,
    FileToCompressTooLong(usize),
}

impl fmt::Display for PXError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IOError(_) => write!(f, "An IO error happened"),
            Self::InvalidHeaderMagic(value) => write!(f, "The header is invalid. It should either be PKDPX or AT4PX. The actual value of this header (in base 10) is {:?}", value),
            Self::InvalidDecompressedLength => write!(f, "The decompressed lenght doesn't correspond to what is indicated in the file"),
            Self::FileToCompressTooLong(lenght) => write!(f, "The file to compress is too long (real size: {}, max size: 256*256)", lenght)
        }
    }
}

impl From<io::Error> for PXError {
    fn from(err: io::Error) -> Self {
        Self::IOError(err)
    }
}

#[derive(Debug)]
struct ControlFlags {
    value: [u8; 9],
}

impl ControlFlags {
    fn new(value: [u8; 9]) -> ControlFlags {
        ControlFlags { value }
    }

    fn find(&self, nb_high: u8) -> Option<usize> {
        for v in 0..self.value.len() {
            if self.value[v] == nb_high {
                return Some(v);
            }
        }
        None
    }
}

fn px_read_u16<T: Read>(file: &mut T) -> Result<u16, PXError> {
    let mut buf = [0; 2];
    file.read_exact(&mut buf)?;
    Ok(u16::from_le_bytes(buf))
}

fn px_read_u32<T: Read>(file: &mut T) -> Result<u32, PXError> {
    let mut buf = [0; 4];
    file.read_exact(&mut buf)?;
    Ok(u32::from_le_bytes(buf))
}

fn px_read_u8<T: Read>(file: &mut T) -> Result<u8, PXError> {
    let mut buf = [0];
    file.read_exact(&mut buf)?;
    Ok(buf[0])
}

/// decompress a pkdpx or at4px file. It take as input a Bytes buffer, and return a decompressed buffer (or an error)
///
/// If atomatically determine if it is a pkdpx or an at4px based on the header
/// If the file isn't the good lenght, it check if what is missing is a padding of a sir0. If it isn't, it return an error.

pub fn decompress_px<F: Read + Seek>(mut file: F) -> Result<Vec<u8>, PXError> {
    debug!("decompressing a px-compressed file file");
    file.seek(SeekFrom::Start(0))?;
    let mut header_5 = [0; 5];
    file.read_exact(&mut header_5)?;

    let container_lenght = px_read_u16(&mut file)?;

    let mut control_flags_buffer = [0; 9];
    file.read_exact(&mut control_flags_buffer)?;
    let control_flags = ControlFlags::new(control_flags_buffer);

    if &header_5 == b"PKDPX" {
        let decompressed_lenght = px_read_u32(&mut file)?;
        Ok(decompress_px_raw(
            file,
            control_flags,
            decompressed_lenght,
            container_lenght,
            20,
        )?)
    } else if &header_5 == b"AT4PX" {
        let decompressed_lenght = px_read_u16(&mut file)? as u32;
        Ok(decompress_px_raw(
            file,
            control_flags,
            decompressed_lenght,
            container_lenght,
            18,
        )?)
    } else {
        Err(PXError::InvalidHeaderMagic(header_5))
    }
}

fn decompress_px_raw<T: Read + Seek>(
    mut file: T,
    control_flags: ControlFlags,
    decompressed_lenght: u32,
    container_lenght: u16,
    header_lenght: u64,
) -> Result<Vec<u8>, PXError> {
    let mut result = Vec::new();
    let current_file_position = file.seek(SeekFrom::Current(0))?;
    let current_file_len = file.seek(SeekFrom::Start(0))?;
    let mut raw_file = Partition::new(
        file,
        current_file_position,
        current_file_len - current_file_position,
    )
    .unwrap();

    trace!("starting decompression ...");
    'main: loop {
        let mut bit_num = 0;
        let byte_info = px_read_u8(&mut raw_file)?;
        trace!("command byte: 0x{:x}", byte_info);
        while bit_num < 8 {
            let this_bit = get_bit(byte_info, bit_num).unwrap();
            let this_byte = px_read_u8(&mut raw_file)?;

            if this_bit {
                trace!("bit is 1: pushing 0x{:2x}", this_byte);
                result.push(this_byte);
            } else {
                let nb_high: u8 = this_byte >> 4;
                let nb_low: u8 = this_byte << 4 >> 4;
                match control_flags.find(nb_high) {
                    Some(ctrlflagindex) => {
                        let byte_to_add = match ctrlflagindex {
                            0 => {
                                let byte1 = (nb_low << 4) + nb_low;
                                (byte1, byte1)
                            }
                            _ => {
                                let mut nybbleval = nb_low;
                                match ctrlflagindex {
                                    1 => nybbleval += 1,
                                    5 => nybbleval -= 1,
                                    _ => (),
                                };
                                let mut nybbles = (nybbleval, nybbleval, nybbleval, nybbleval);
                                match ctrlflagindex {
                                    1 => nybbles.0 -= 1,
                                    2 => nybbles.1 -= 1,
                                    3 => nybbles.2 -= 1,
                                    4 => nybbles.3 -= 1,
                                    5 => nybbles.0 += 1,
                                    6 => nybbles.1 += 1,
                                    7 => nybbles.2 += 1,
                                    8 => nybbles.3 += 1,
                                    _ => panic!(),
                                }
                                ((nybbles.0 << 4) + nybbles.1, (nybbles.2 << 4) + nybbles.3)
                            }
                        };
                        trace!("bit is 0: ctrlflagindex is {:x}, nb_high is {:x}, nb_low is {:x}, adding 0x{:2x}{:2x}", ctrlflagindex, nb_high, nb_low, byte_to_add.0, byte_to_add.1);
                        result.push(byte_to_add.0);
                        result.push(byte_to_add.1);
                    }
                    None => {
                        let new_byte = px_read_u8(&mut raw_file)?;
                        let offset_rel: i16 =
                            -0x1000 + (((nb_low as i16) * 256) + (new_byte as i16));
                        let offset = (offset_rel as i32) + (result.len() as i32);
                        let lenght = (nb_high as i32) + 3;
                        trace!("bit is 0: pushing from past, relative offset is {}, lenght is {} (nb_low:{}, nb_high:{}, new_byte:0x{:2x})", offset_rel, lenght, nb_low, nb_high, new_byte);
                        // the old, good looking code
                        /*result.seek(offset as u64);
                        for c in result.read(lenght as u64)? {
                            result.add_a_byte(c)?;
                        }*/
                        //TODO: check for panic
                        for c in offset..(offset + lenght) {
                            result.push(result[c as usize])
                        }
                    }
                }
            };
            bit_num += 1;
            if result.len() >= decompressed_lenght as usize {
                break 'main;
            };
        }
        trace!("current output size : {}", result.len());
    }
    trace!("decoding loop finished.");
    trace!(
        "expected container lenght: {}, read: {}",
        container_lenght,
        raw_file.seek(SeekFrom::Current(0))? + 20
    );
    trace!(
        "expected decompressed lenght: {}, real decompressed lenght: {}",
        decompressed_lenght,
        result.len()
    );
    if container_lenght as u64 != raw_file.seek(SeekFrom::Current(0))? + header_lenght {
        return Err(PXError::InvalidDecompressedLength);
    };
    Ok(result)
}

/// check if a file is a px-compressed filed (PKDPX or AT4PX) .
/// return true if it is one, false otherwise.
///
/// It doesn't do extensive test and don't guaranty that the file is a valid PKDPX (only check the header)
/// Also doesn't save the position of the cursor in the file
pub fn is_px<F: Read + Seek>(file: &mut F) -> Result<bool, PXError> {
    if file.seek(SeekFrom::End(0))? < 4 {
        return Ok(false);
    };

    file.seek(SeekFrom::Start(0))?;

    let mut header_5 = [0; 5];
    file.read_exact(&mut header_5)?;

    if &header_5 == b"PKDPX" {
        return Ok(true);
    };
    if &header_5 == b"AT4PX" {
        return Ok(true);
    };
    Ok(false)
}

/// use a naive compression algoritm to compress the input to a PKDPX file
pub fn naive_compression<F: Read + Seek>(mut file: F) -> Result<Vec<u8>, PXError> {
    let decompressed_size = file.seek(SeekFrom::End(0))?;
    file.seek(SeekFrom::Start(0))?;

    let mut result = Vec::new();
    // header
    result.append(&mut b"PKDPX".to_vec());
    // container_lenght
    result.append(&mut u16::to_le_bytes(0).to_vec()); //TODO: rewrite
                                                      // control flags
    for _ in 0..9 {
        result.push(0);
    }
    // decompressed lenght
    result.append(&mut u32::to_le_bytes(decompressed_size as u32).to_vec());

    let mut loop_nb = 0;
    loop {
        if loop_nb % 8 == 0 {
            result.push(0xFF);
        };
        result.push(px_read_u8(&mut file)?);

        if file.seek(SeekFrom::Current(0))? >= decompressed_size {
            break;
        };
        loop_nb += 1;
    }

    let container_lenght = result.len();
    while result.len() % 16 != 0 {
        result.push(0xAA);
    }

    if container_lenght > (core::u16::MAX as usize) {
        return Err(PXError::FileToCompressTooLong(container_lenght));
    };

    let lenght_splice = u16::to_le_bytes(container_lenght as u16);
    result[5] = lenght_splice[0];
    result[6] = lenght_splice[1];

    Ok(result)
}
