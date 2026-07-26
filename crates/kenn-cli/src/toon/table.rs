//! The table writer: `write_table` plus the serializers that lay out the
//! top-level object — its scalar fields (dropped; `render` prints those) and its
//! array field as `items[N]{cols}:` + rows.
//!
//! `BlockSerializer` handles a value on its own line, `ObjectSerializer` the
//! top-level object's fields, and `ArraySerializer` the header/data split.

use std::io::{self, Write};

use serde::ser::{self, Impossible, Serialize};

use super::element::{Elem, ElemSerializer, KeySerializer};
use super::grammar::{
    array_stem, encode_key, line, nested, write_f64, write_indent, write_key, write_str_token,
    Error,
};

/// Stream a flat-list value as TOON to `out`, or return `Err` if it isn't the
/// flat-table shape (a nested object or array-of-nested-objects). Because a
/// nested payload is only discovered mid-write, the caller decides whether to
/// stream straight to stdout (having first confirmed the shape against a
/// discarding sink) or handle the `Err` some other way — see `render::emit`.
pub fn write_table<W: Write + 'static, T: Serialize + ?Sized>(
    out: &mut W,
    value: &T,
) -> Result<(), Error> {
    value.serialize(BlockSerializer {
        out,
        depth: 0,
        label: None,
        allow_object: true,
        skip_scalar: false,
    })
}

// --- BlockSerializer: the top-level object (or a scalar / array field) ------

/// Serializes a value on its own line(s): the top-level result object, or a
/// field's scalar/array value. A NESTED object (`allow_object` false) errors so
/// the caller renders JSON instead. A `skip_scalar` field is a top-level scalar
/// of the wrapper (e.g. `next`) — NOT part of the table; `render` prints those,
/// so the serializer drops them.
struct BlockSerializer<'a> {
    out: &'a mut (dyn Write + 'static),
    depth: usize,
    label: Option<String>,
    allow_object: bool,
    skip_scalar: bool,
}

impl BlockSerializer<'_> {
    /// Write one scalar on its own line — `{indent}{key}: <value>` (or bare at
    /// top level) — formatting the value straight to the writer via the closure.
    /// A wrapper's scalar field (`skip_scalar`) is dropped: TOON emits only the
    /// table, and `render` prints `next`/meta separately.
    fn scalar(
        self,
        write_value: impl FnOnce(&mut dyn Write) -> io::Result<()>,
    ) -> Result<(), Error> {
        if self.skip_scalar {
            return Ok(());
        }
        write_indent(self.out, self.depth)?;
        if let Some(k) = &self.label {
            write_key(self.out, k)?;
            self.out.write_all(b": ")?;
        }
        write_value(self.out)?;
        self.out.write_all(b"\n")?;
        Ok(())
    }
}

impl<'a> serde::Serializer for BlockSerializer<'a> {
    type Ok = ();
    type Error = Error;
    type SerializeSeq = ArraySerializer<'a>;
    type SerializeTuple = ArraySerializer<'a>;
    type SerializeTupleStruct = ArraySerializer<'a>;
    type SerializeTupleVariant = Impossible<(), Error>;
    type SerializeMap = ObjectSerializer<'a>;
    type SerializeStruct = ObjectSerializer<'a>;
    type SerializeStructVariant = Impossible<(), Error>;

    fn serialize_bool(self, v: bool) -> Result<(), Error> {
        self.scalar(|w| write!(w, "{v}"))
    }
    fn serialize_i8(self, v: i8) -> Result<(), Error> {
        self.scalar(|w| write!(w, "{v}"))
    }
    fn serialize_i16(self, v: i16) -> Result<(), Error> {
        self.scalar(|w| write!(w, "{v}"))
    }
    fn serialize_i32(self, v: i32) -> Result<(), Error> {
        self.scalar(|w| write!(w, "{v}"))
    }
    fn serialize_i64(self, v: i64) -> Result<(), Error> {
        self.scalar(|w| write!(w, "{v}"))
    }
    fn serialize_u8(self, v: u8) -> Result<(), Error> {
        self.scalar(|w| write!(w, "{v}"))
    }
    fn serialize_u16(self, v: u16) -> Result<(), Error> {
        self.scalar(|w| write!(w, "{v}"))
    }
    fn serialize_u32(self, v: u32) -> Result<(), Error> {
        self.scalar(|w| write!(w, "{v}"))
    }
    fn serialize_u64(self, v: u64) -> Result<(), Error> {
        self.scalar(|w| write!(w, "{v}"))
    }
    fn serialize_f32(self, v: f32) -> Result<(), Error> {
        self.scalar(|w| write_f64(w, v.into()))
    }
    fn serialize_f64(self, v: f64) -> Result<(), Error> {
        self.scalar(|w| write_f64(w, v))
    }
    fn serialize_char(self, v: char) -> Result<(), Error> {
        self.serialize_str(v.encode_utf8(&mut [0u8; 4]))
    }
    fn serialize_str(self, v: &str) -> Result<(), Error> {
        self.scalar(|w| write_str_token(w, v))
    }
    fn serialize_bytes(self, _v: &[u8]) -> Result<(), Error> {
        Err(nested())
    }
    fn serialize_none(self) -> Result<(), Error> {
        self.scalar(|w| w.write_all(b"null"))
    }
    fn serialize_some<T: ?Sized + Serialize>(self, v: &T) -> Result<(), Error> {
        v.serialize(self)
    }
    fn serialize_unit(self) -> Result<(), Error> {
        self.scalar(|w| w.write_all(b"null"))
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<(), Error> {
        self.scalar(|w| w.write_all(b"null"))
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _idx: u32,
        variant: &'static str,
    ) -> Result<(), Error> {
        self.scalar(|w| write_str_token(w, variant))
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        v: &T,
    ) -> Result<(), Error> {
        v.serialize(self)
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        _idx: u32,
        _variant: &'static str,
        _v: &T,
    ) -> Result<(), Error> {
        Err(nested())
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<ArraySerializer<'a>, Error> {
        Ok(ArraySerializer {
            out: self.out,
            depth: self.depth,
            label: self.label,
            len: len.unwrap_or(0),
            state: ArrayState::Start,
        })
    }
    fn serialize_tuple(self, len: usize) -> Result<ArraySerializer<'a>, Error> {
        self.serialize_seq(Some(len))
    }
    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        len: usize,
    ) -> Result<ArraySerializer<'a>, Error> {
        self.serialize_seq(Some(len))
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _idx: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Impossible<(), Error>, Error> {
        Err(nested())
    }
    fn serialize_map(self, _len: Option<usize>) -> Result<ObjectSerializer<'a>, Error> {
        // Only the top-level object is a TOON table; a nested object errors so
        // the caller falls back to JSON.
        if !self.allow_object {
            return Err(nested());
        }
        Ok(ObjectSerializer {
            out: self.out,
            depth: self.depth,
            next_key: None,
        })
    }
    fn serialize_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<ObjectSerializer<'a>, Error> {
        self.serialize_map(None)
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _idx: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Impossible<(), Error>, Error> {
        Err(nested())
    }
}

// --- ObjectSerializer: the top object, one field per line -------------------

struct ObjectSerializer<'a> {
    out: &'a mut (dyn Write + 'static),
    depth: usize,
    next_key: Option<String>,
}

impl ObjectSerializer<'_> {
    fn field<T: ?Sized + Serialize>(&mut self, key: String, value: &T) -> Result<(), Error> {
        // The top-level object's fields: an array renders as a table; a scalar
        // (`next`, `targets`, …) is the wrapper's meta and is dropped here —
        // `render` prints it. A nested object errors (→ JSON).
        value.serialize(BlockSerializer {
            out: self.out,
            depth: self.depth,
            label: Some(key),
            allow_object: false,
            skip_scalar: true,
        })
    }
}

impl ser::SerializeStruct for ObjectSerializer<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Error> {
        self.field(key.to_string(), value)
    }
    fn end(self) -> Result<(), Error> {
        Ok(())
    }
}

impl ser::SerializeMap for ObjectSerializer<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Result<(), Error> {
        self.next_key = Some(key.serialize(KeySerializer)?);
        Ok(())
    }
    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Error> {
        let key = self
            .next_key
            .take()
            .ok_or_else(|| Error::msg("map value before key"))?;
        self.field(key, value)
    }
    fn end(self) -> Result<(), Error> {
        Ok(())
    }
}

// --- ArraySerializer: the header/data split (or an inline scalar array) ------

enum ArrayState {
    Start,
    Inline(Vec<String>), // scalar array — one line at end()
    /// Header written; rows stream. Carries the header's field names so every
    /// later row can be checked against them — a streaming encoder cannot look
    /// ahead, so uniformity must be verified row by row rather than assumed.
    Tabular(Vec<String>),
}

struct ArraySerializer<'a> {
    out: &'a mut (dyn Write + 'static),
    depth: usize,
    label: Option<String>,
    len: usize,
    state: ArrayState,
}

impl ArraySerializer<'_> {
    fn push_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Error> {
        match &mut self.state {
            ArrayState::Start => match value.serialize(ElemSerializer)? {
                Elem::Prim(tok) => self.state = ArrayState::Inline(vec![tok]),
                Elem::Row(pairs) => {
                    let cols: Vec<String> = pairs.iter().map(|(k, _)| encode_key(k)).collect();
                    line(
                        self.out,
                        self.depth,
                        &format!(
                            "{}{{{}}}:",
                            array_stem(self.label.as_deref(), self.len),
                            cols.join(",")
                        ),
                    )?;
                    let keys: Vec<String> = pairs.iter().map(|(k, _)| k.clone()).collect();
                    let row: Vec<String> = pairs.into_iter().map(|(_, v)| v).collect();
                    line(self.out, self.depth + 1, &row.join(","))?;
                    self.state = ArrayState::Tabular(keys);
                }
            },
            ArrayState::Inline(toks) => match value.serialize(ElemSerializer)? {
                Elem::Prim(tok) => toks.push(tok),
                // A row after a scalar is not a uniform list; dropping it would
                // print `[N]` over fewer than N values.
                Elem::Row(_) => return Err(nested()),
            },
            ArrayState::Tabular(header) => match value.serialize(ElemSerializer)? {
                Elem::Row(pairs) => {
                    // The header came from the FIRST row. A later row with a
                    // different field set (an omitted `skip_serializing_if`
                    // field, say) would be comma-joined under the wrong columns
                    // and read as plausible — so reject the shape and let the
                    // caller fall back to JSON.
                    if pairs.len() != header.len()
                        || pairs.iter().zip(header.iter()).any(|((k, _), h)| k != h)
                    {
                        return Err(nested());
                    }
                    let row: Vec<String> = pairs.into_iter().map(|(_, v)| v).collect();
                    line(self.out, self.depth + 1, &row.join(","))?;
                }
                Elem::Prim(_) => return Err(nested()),
            },
        }
        Ok(())
    }

    fn finish(self) -> Result<(), Error> {
        match self.state {
            ArrayState::Start => line(
                self.out,
                self.depth,
                &format!("{}:", array_stem(self.label.as_deref(), self.len)),
            ),
            ArrayState::Inline(toks) => line(
                self.out,
                self.depth,
                &format!(
                    "{}: {}",
                    array_stem(self.label.as_deref(), self.len),
                    toks.join(",")
                ),
            ),
            ArrayState::Tabular(_) => Ok(()),
        }
    }
}

impl ser::SerializeSeq for ArraySerializer<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Error> {
        self.push_element(value)
    }
    fn end(self) -> Result<(), Error> {
        self.finish()
    }
}
impl ser::SerializeTuple for ArraySerializer<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Error> {
        self.push_element(value)
    }
    fn end(self) -> Result<(), Error> {
        self.finish()
    }
}
impl ser::SerializeTupleStruct for ArraySerializer<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Error> {
        self.push_element(value)
    }
    fn end(self) -> Result<(), Error> {
        self.finish()
    }
}
