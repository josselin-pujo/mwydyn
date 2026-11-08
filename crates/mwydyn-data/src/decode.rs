//! Decoding into [`Value`] under the canonical profile's rejection
//! rules (spec §1).

use crate::{key_is_hygienic, map_sort_key, DecodeError, DecodeOptions, Value};

/// Decode one value; trailing bytes are an error.
pub fn decode(bytes: &[u8], opts: &DecodeOptions) -> Result<Value, DecodeError> {
    if bytes.is_empty() {
        return Err(DecodeError::Empty);
    }
    let mut r = Reader { b: bytes, pos: 0 };
    let v = read_value(&mut r, opts, 0)?;
    if r.pos != r.b.len() {
        return Err(DecodeError::TrailingBytes);
    }
    Ok(v)
}

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn u8(&mut self) -> Result<u8, DecodeError> {
        if self.pos >= self.b.len() {
            return Err(DecodeError::Malformed("truncated"));
        }
        let v = self.b[self.pos];
        self.pos += 1;
        Ok(v)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        if self.b.len() - self.pos < n {
            return Err(DecodeError::Malformed("truncated"));
        }
        let s = &self.b[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    /// Returns (major, additional-info, argument). For floats the
    /// argument carries the raw bits.
    fn head(&mut self) -> Result<(u8, u8, u64), DecodeError> {
        let ib = self.u8()?;
        let major = ib >> 5;
        let ai = ib & 0x1f;
        let arg = match ai {
            0..=23 => ai as u64,
            24 => self.u8()? as u64,
            25 => u16::from_be_bytes(self.take(2)?.try_into().unwrap()) as u64,
            26 => u32::from_be_bytes(self.take(4)?.try_into().unwrap()) as u64,
            27 => u64::from_be_bytes(self.take(8)?.try_into().unwrap()),
            28..=30 => {
                return Err(DecodeError::Malformed("reserved additional info"))
            }
            _ => return Err(DecodeError::Indefinite),
        };
        Ok((major, ai, arg))
    }
}

fn make_float(f: f64) -> Result<Value, DecodeError> {
    if f.is_finite() {
        Ok(Value::Float(f))
    } else {
        Err(DecodeError::NonFiniteFloat)
    }
}

fn int_from_major0(arg: u64) -> Result<Value, DecodeError> {
    if arg <= i64::MAX as u64 {
        Ok(Value::Int(arg as i64))
    } else {
        Value::big_pos(arg.to_be_bytes().to_vec()).map_err(|_| DecodeError::BadBignum)
    }
}

fn int_from_major1(arg: u64) -> Result<Value, DecodeError> {
    if arg <= i64::MAX as u64 {
        Ok(Value::Int(-1 - arg as i64))
    } else {
        Value::big_neg(arg.to_be_bytes().to_vec()).map_err(|_| DecodeError::BadBignum)
    }
}

fn half_to_f64(h: u16) -> f64 {
    let sign = (h >> 15) & 1;
    let exp = ((h >> 10) & 0x1f) as i32;
    let frac = (h & 0x3ff) as f64;
    let val = if exp == 0 {
        frac * 2f64.powi(-24)
    } else if exp == 31 {
        if frac == 0.0 {
            f64::INFINITY
        } else {
            f64::NAN
        }
    } else {
        (frac / 1024.0 + 1.0) * 2f64.powi(exp - 15)
    };
    if sign == 1 {
        -val
    } else {
        val
    }
}

fn read_value(
    r: &mut Reader<'_>,
    opts: &DecodeOptions,
    depth: usize,
) -> Result<Value, DecodeError> {
    if depth > opts.max_depth {
        return Err(DecodeError::TooDeep);
    }
    let (major, ai, arg) = r.head()?;
    match major {
        0 => int_from_major0(arg),
        1 => int_from_major1(arg),
        2 => {
            let bytes = r.take(arg as usize)?;
            Ok(Value::Bytes(bytes.to_vec()))
        }
        3 => {
            let bytes = r.take(arg as usize)?;
            let s = std::str::from_utf8(bytes)
                .map_err(|_| DecodeError::Malformed("bad utf8"))?;
            Ok(Value::Text(s.to_string()))
        }
        4 => {
            let mut items = Vec::new();
            for _ in 0..arg {
                items.push(read_value(r, opts, depth + 1)?);
            }
            Ok(Value::Array(items))
        }
        5 => {
            let mut pairs: Vec<(String, Value)> = Vec::new();
            for _ in 0..arg {
                let key = read_value(r, opts, depth + 1)?;
                let key = match key {
                    Value::Text(s) => s,
                    _ => return Err(DecodeError::Malformed("non-string map key")),
                };
                if !key_is_hygienic(&key, opts.hygiene) {
                    return Err(DecodeError::BadKeyHygiene);
                }
                let val = read_value(r, opts, depth + 1)?;
                pairs.push((key, val));
            }
            pairs.sort_by(|a, b| map_sort_key(&a.0).cmp(&map_sort_key(&b.0)));
            for w in pairs.windows(2) {
                if w[0].0 == w[1].0 {
                    return Err(DecodeError::DuplicateKey(w[0].0.clone()));
                }
            }
            Ok(Value::map_unchecked(pairs))
        }
        6 => match arg {
            2 | 3 => {
                let (m, _ai, len) = r.head()?;
                if m != 2 {
                    return Err(DecodeError::Malformed("bignum payload not bytes"));
                }
                let bytes = r.take(len as usize)?.to_vec();
                if bytes.is_empty() || bytes[0] == 0 {
                    return Err(DecodeError::BadBignum);
                }
                if arg == 2 {
                    Ok(Value::BigPos(bytes))
                } else {
                    Ok(Value::BigNeg(bytes))
                }
            }
            1001 => {
                let (m, _ai, inner) = r.head()?;
                match m {
                    0 if inner <= i64::MAX as u64 => Ok(Value::Timestamp(inner as i64)),
                    1 if inner <= i64::MAX as u64 => Ok(Value::Timestamp(-1 - inner as i64)),
                    _ => Err(DecodeError::Malformed("timestamp payload not an int")),
                }
            }
            1002 => {
                // BlobRef: {size, digest, mediaType} in canonical order.
                // Shape is a data-model fact (profile v0.1, round 12);
                // hash verification is a verification-point concern.
                let inner = read_value(r, opts, depth + 1)?;
                let Value::Map(pairs) = inner else {
                    return Err(DecodeError::BadBlobRef);
                };
                if pairs.len() != 3
                    || pairs[0].0 != "size"
                    || pairs[1].0 != "digest"
                    || pairs[2].0 != "mediaType"
                {
                    return Err(DecodeError::BadBlobRef);
                }
                let size = match &pairs[0].1 {
                    Value::Int(n) if *n >= 0 => *n as u64,
                    Value::BigPos(m) if m.len() <= 8 => {
                        let mut buf = [0u8; 8];
                        buf[8 - m.len()..].copy_from_slice(m);
                        u64::from_be_bytes(buf)
                    }
                    _ => return Err(DecodeError::BadBlobRef),
                };
                let Value::Text(digest) = &pairs[1].1 else {
                    return Err(DecodeError::BadBlobRef);
                };
                let Value::Text(media_type) = &pairs[2].1 else {
                    return Err(DecodeError::BadBlobRef);
                };
                Value::validate_blob_ref(digest, media_type)
                    .map_err(|_| DecodeError::BadBlobRef)?;
                Ok(Value::BlobRef {
                    digest: digest.clone(),
                    media_type: media_type.clone(),
                    size,
                })
            }
            other => Err(DecodeError::UnknownTag(other)),
        },
        7 => match ai {
            20 => Ok(Value::Bool(false)),
            21 => Ok(Value::Bool(true)),
            22 => Ok(Value::Null),
            25 => make_float(half_to_f64(arg as u16)),
            26 => make_float(f32::from_bits(arg as u32) as f64),
            27 => make_float(f64::from_bits(arg)),
            _ => Err(DecodeError::Malformed(
                "simple value not in the data model",
            )),
        },
        _ => unreachable!(),
    }
}
