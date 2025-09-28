//! Bencode parser without dependency on string encodings.
//!
//! A lot of libraries for bencode format are only suitable for the case where every byte-string
//! entry only consists of valid UTF-8 codepoints, because they only parses [`String`]. However,
//! they fail to parse real-world torrent files as they often contains raw bytes. This simple
//! parser correctly handles general cases.
//!
//! This module is only meant to be used within this project instead of for general use, because it
//! does not provide a formal interface, and does not implement standard-conforming error
//! handling. For example, the `into`-`try_from` roundtrip results in a different bencode object
//! since `TryFrom<&[u8]>` adds an outer [`BencodeObject::List`] to the object.

use anyhow::Result;

#[inline]
fn chr(x: u8) -> Result<char> {
    char::from_u32(x as u32).ok_or(anyhow::anyhow!("unknown character {}", x))
}

#[inline]
fn to_digit(x: char) -> Result<usize> {
    x.to_digit(10)
        .map(|x| x as usize)
        .ok_or(anyhow::anyhow!("cannot convert {} to number", x))
}

#[derive(Debug)]
pub(crate) enum BencodeObject {
    None,
    Integer(String),
    /// If size is zero, then vec is None.
    Bytes(usize, Option<Vec<u8>>),
    List(Vec<BencodeObject>),
    /// Stores key-value pairs.
    Dictionary(Vec<(BencodeObject, BencodeObject)>),
}

impl TryFrom<&[u8]> for BencodeObject {
    type Error = anyhow::Error;

    /// Deserializes bencode objects. The root is a [`BencodeObject::List`].
    fn try_from(value: &[u8]) -> Result<Self> {
        let mut parser = BencodeParser::new();
        for &x in value {
            parser.next(x)?;
        }
        Ok(parser.stack.pop().unwrap())
    }
}

impl Into<Vec<u8>> for BencodeObject {
    /// Serializes bencode objects.
    fn into(self) -> Vec<u8> {
        let mut result = Vec::new();
        match self {
            BencodeObject::None => (),
            BencodeObject::Integer(value) => {
                result.push('i' as u8);
                result.extend(value.as_bytes());
                result.push('e' as u8);
            }
            BencodeObject::Bytes(size, value) => {
                result.extend(size.to_string().as_bytes());
                result.push(':' as u8);
                if let Some(value) = value {
                    result.extend(value);
                }
            }
            BencodeObject::List(list) => {
                result.push('l' as u8);
                for item in list {
                    result.extend(Into::<Vec<u8>>::into(item));
                }
                result.push('e' as u8);
            }
            BencodeObject::Dictionary(dict) => {
                result.push('d' as u8);
                for (k, v) in dict {
                    result.extend(Into::<Vec<u8>>::into(k));
                    result.extend(Into::<Vec<u8>>::into(v));
                }
                result.push('e' as u8);
            }
        }
        result
    }
}

struct BencodeParser {
    stack: Vec<BencodeObject>,
}

impl BencodeParser {
    fn new() -> Self {
        Self {
            stack: vec![BencodeObject::List(Vec::new())],
        }
    }
    fn push(&mut self) -> Result<()> {
        let last = self.stack.pop().unwrap();
        match self.stack.last_mut().unwrap() {
            BencodeObject::List(list) => {
                list.push(last);
            }
            BencodeObject::Dictionary(dict) => {
                let target = dict.last_mut();
                if !target.is_some_and(|target| {
                    matches!(target.1, BencodeObject::None)
                        || matches!(target.0, BencodeObject::None)
                }) {
                    dict.push((BencodeObject::None, BencodeObject::None));
                }

                let target = unsafe { dict.last_mut().unwrap_unchecked() };

                if matches!(target.0, BencodeObject::None) {
                    target.0 = last;
                } else if matches!(target.1, BencodeObject::None) {
                    target.1 = last;
                } else {
                    unreachable!();
                }
            }
            _ => unreachable!(),
        }
        Ok(())
    }
    fn next(&mut self, x: u8) -> Result<()> {
        let mut push = false;
        if let Some(curr_obj) = self.stack.last_mut() {
            match curr_obj {
                BencodeObject::None => panic!(),
                BencodeObject::Integer(value) => {
                    let x = chr(x)?;
                    if x == 'e' {
                        push = true;
                    } else {
                        value.extend_one(x);
                    }
                }
                BencodeObject::Bytes(size, value) => {
                    if let Some(value) = value {
                        value.push(x);
                        if value.len() == *size {
                            push = true;
                        }
                    } else {
                        let x = chr(x)?;
                        if x == ':' {
                            if *size == 0 {
                                push = true;
                            } else {
                                *value = Some(Vec::new());
                            }
                        } else {
                            let x = to_digit(x)?;
                            *size = *size * 10 + x;
                        }
                    }
                }
                BencodeObject::List(_) | BencodeObject::Dictionary(_) => {
                    let x = chr(x)?;
                    match x {
                        'e' => {
                            push = true;
                        }
                        'i' => {
                            self.stack.push(BencodeObject::Integer(String::new()));
                        }
                        '0'..='9' => {
                            self.stack.push(BencodeObject::Bytes(to_digit(x)?, None));
                        }
                        'l' => {
                            self.stack.push(BencodeObject::List(Vec::new()));
                        }
                        'd' => {
                            self.stack.push(BencodeObject::Dictionary(Vec::new()));
                        }
                        _ => return Err(anyhow::anyhow!("syntax error near {}", x)),
                    }
                }
            }
        }

        if push {
            self.push()?;
        }

        Ok(())
    }
}
