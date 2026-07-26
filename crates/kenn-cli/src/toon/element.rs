//! Element classification: what one array element (and one table cell) is
//! allowed to be.
//!
//! `ElemSerializer` decides whether an element is a scalar (inline array) or a
//! flat object (a table row, its field names becoming the header); `CellSerializer`
//! requires each row field to be a primitive, and `KeySerializer` reduces a map
//! key to a string. Anything nested errors, so the caller falls back to JSON.

use serde::ser::{self, Impossible, Serialize};

use super::grammar::{nested, tok_bool, tok_f64, tok_str, Error};

// --- ElemSerializer: one array element must be a scalar or a flat object -----

pub(super) enum Elem {
    Prim(String),
    Row(Vec<(String, String)>),
}

pub(super) struct ElemSerializer;

impl serde::Serializer for ElemSerializer {
    type Ok = Elem;
    type Error = Error;
    type SerializeSeq = Impossible<Elem, Error>;
    type SerializeTuple = Impossible<Elem, Error>;
    type SerializeTupleStruct = Impossible<Elem, Error>;
    type SerializeTupleVariant = Impossible<Elem, Error>;
    type SerializeMap = RowBuilder;
    type SerializeStruct = RowBuilder;
    type SerializeStructVariant = Impossible<Elem, Error>;

    fn serialize_bool(self, v: bool) -> Result<Elem, Error> {
        Ok(Elem::Prim(tok_bool(v)))
    }
    fn serialize_i8(self, v: i8) -> Result<Elem, Error> {
        Ok(Elem::Prim(v.to_string()))
    }
    fn serialize_i16(self, v: i16) -> Result<Elem, Error> {
        Ok(Elem::Prim(v.to_string()))
    }
    fn serialize_i32(self, v: i32) -> Result<Elem, Error> {
        Ok(Elem::Prim(v.to_string()))
    }
    fn serialize_i64(self, v: i64) -> Result<Elem, Error> {
        Ok(Elem::Prim(v.to_string()))
    }
    fn serialize_u8(self, v: u8) -> Result<Elem, Error> {
        Ok(Elem::Prim(v.to_string()))
    }
    fn serialize_u16(self, v: u16) -> Result<Elem, Error> {
        Ok(Elem::Prim(v.to_string()))
    }
    fn serialize_u32(self, v: u32) -> Result<Elem, Error> {
        Ok(Elem::Prim(v.to_string()))
    }
    fn serialize_u64(self, v: u64) -> Result<Elem, Error> {
        Ok(Elem::Prim(v.to_string()))
    }
    fn serialize_f32(self, v: f32) -> Result<Elem, Error> {
        Ok(Elem::Prim(tok_f64(v.into())))
    }
    fn serialize_f64(self, v: f64) -> Result<Elem, Error> {
        Ok(Elem::Prim(tok_f64(v)))
    }
    fn serialize_char(self, v: char) -> Result<Elem, Error> {
        Ok(Elem::Prim(tok_str(&v.to_string())))
    }
    fn serialize_str(self, v: &str) -> Result<Elem, Error> {
        Ok(Elem::Prim(tok_str(v)))
    }
    fn serialize_bytes(self, _v: &[u8]) -> Result<Elem, Error> {
        Err(nested())
    }
    fn serialize_none(self) -> Result<Elem, Error> {
        Ok(Elem::Prim("null".into()))
    }
    fn serialize_some<T: ?Sized + Serialize>(self, v: &T) -> Result<Elem, Error> {
        v.serialize(self)
    }
    fn serialize_unit(self) -> Result<Elem, Error> {
        Ok(Elem::Prim("null".into()))
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<Elem, Error> {
        Ok(Elem::Prim("null".into()))
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _idx: u32,
        variant: &'static str,
    ) -> Result<Elem, Error> {
        Ok(Elem::Prim(tok_str(variant)))
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        v: &T,
    ) -> Result<Elem, Error> {
        v.serialize(self)
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        _idx: u32,
        _variant: &'static str,
        _v: &T,
    ) -> Result<Elem, Error> {
        Err(nested())
    }
    fn serialize_seq(self, _len: Option<usize>) -> Result<Impossible<Elem, Error>, Error> {
        Err(nested())
    }
    fn serialize_tuple(self, _len: usize) -> Result<Impossible<Elem, Error>, Error> {
        Err(nested())
    }
    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Impossible<Elem, Error>, Error> {
        Err(nested())
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _idx: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Impossible<Elem, Error>, Error> {
        Err(nested())
    }
    fn serialize_map(self, _len: Option<usize>) -> Result<RowBuilder, Error> {
        Ok(RowBuilder {
            pairs: Vec::new(),
            next_key: None,
        })
    }
    fn serialize_struct(self, _name: &'static str, len: usize) -> Result<RowBuilder, Error> {
        Ok(RowBuilder {
            pairs: Vec::with_capacity(len),
            next_key: None,
        })
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _idx: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Impossible<Elem, Error>, Error> {
        Err(nested())
    }
}

/// Builds a tabular row: each field's value goes through [`CellSerializer`],
/// which errors if the field is itself an object or array (→ not a flat table).
pub(super) struct RowBuilder {
    pairs: Vec<(String, String)>,
    next_key: Option<String>,
}

impl ser::SerializeStruct for RowBuilder {
    type Ok = Elem;
    type Error = Error;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Error> {
        self.pairs
            .push((key.to_string(), value.serialize(CellSerializer)?));
        Ok(())
    }
    fn end(self) -> Result<Elem, Error> {
        Ok(Elem::Row(self.pairs))
    }
}

impl ser::SerializeMap for RowBuilder {
    type Ok = Elem;
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
        self.pairs.push((key, value.serialize(CellSerializer)?));
        Ok(())
    }
    fn end(self) -> Result<Elem, Error> {
        Ok(Elem::Row(self.pairs))
    }
}

// --- CellSerializer: a table cell must be a primitive -----------------------

struct CellSerializer;

impl serde::Serializer for CellSerializer {
    type Ok = String;
    type Error = Error;
    type SerializeSeq = Impossible<String, Error>;
    type SerializeTuple = Impossible<String, Error>;
    type SerializeTupleStruct = Impossible<String, Error>;
    type SerializeTupleVariant = Impossible<String, Error>;
    type SerializeMap = Impossible<String, Error>;
    type SerializeStruct = Impossible<String, Error>;
    type SerializeStructVariant = Impossible<String, Error>;

    fn serialize_bool(self, v: bool) -> Result<String, Error> {
        Ok(tok_bool(v))
    }
    fn serialize_i8(self, v: i8) -> Result<String, Error> {
        Ok(v.to_string())
    }
    fn serialize_i16(self, v: i16) -> Result<String, Error> {
        Ok(v.to_string())
    }
    fn serialize_i32(self, v: i32) -> Result<String, Error> {
        Ok(v.to_string())
    }
    fn serialize_i64(self, v: i64) -> Result<String, Error> {
        Ok(v.to_string())
    }
    fn serialize_u8(self, v: u8) -> Result<String, Error> {
        Ok(v.to_string())
    }
    fn serialize_u16(self, v: u16) -> Result<String, Error> {
        Ok(v.to_string())
    }
    fn serialize_u32(self, v: u32) -> Result<String, Error> {
        Ok(v.to_string())
    }
    fn serialize_u64(self, v: u64) -> Result<String, Error> {
        Ok(v.to_string())
    }
    fn serialize_f32(self, v: f32) -> Result<String, Error> {
        Ok(tok_f64(v.into()))
    }
    fn serialize_f64(self, v: f64) -> Result<String, Error> {
        Ok(tok_f64(v))
    }
    fn serialize_char(self, v: char) -> Result<String, Error> {
        Ok(tok_str(&v.to_string()))
    }
    fn serialize_str(self, v: &str) -> Result<String, Error> {
        Ok(tok_str(v))
    }
    fn serialize_bytes(self, _v: &[u8]) -> Result<String, Error> {
        Err(nested())
    }
    fn serialize_none(self) -> Result<String, Error> {
        Ok("null".into())
    }
    fn serialize_some<T: ?Sized + Serialize>(self, v: &T) -> Result<String, Error> {
        v.serialize(self)
    }
    fn serialize_unit(self) -> Result<String, Error> {
        Ok("null".into())
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<String, Error> {
        Ok("null".into())
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _idx: u32,
        variant: &'static str,
    ) -> Result<String, Error> {
        Ok(tok_str(variant))
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        v: &T,
    ) -> Result<String, Error> {
        v.serialize(self)
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        _idx: u32,
        _variant: &'static str,
        _v: &T,
    ) -> Result<String, Error> {
        Err(nested())
    }
    fn serialize_seq(self, _len: Option<usize>) -> Result<Impossible<String, Error>, Error> {
        Err(nested())
    }
    fn serialize_tuple(self, _len: usize) -> Result<Impossible<String, Error>, Error> {
        Err(nested())
    }
    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Impossible<String, Error>, Error> {
        Err(nested())
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _idx: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Impossible<String, Error>, Error> {
        Err(nested())
    }
    fn serialize_map(self, _len: Option<usize>) -> Result<Impossible<String, Error>, Error> {
        Err(nested())
    }
    fn serialize_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Impossible<String, Error>, Error> {
        Err(nested())
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _idx: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Impossible<String, Error>, Error> {
        Err(nested())
    }
}

// --- KeySerializer: a map key must reduce to a string -----------------------

pub(super) struct KeySerializer;

impl serde::Serializer for KeySerializer {
    type Ok = String;
    type Error = Error;
    type SerializeSeq = Impossible<String, Error>;
    type SerializeTuple = Impossible<String, Error>;
    type SerializeTupleStruct = Impossible<String, Error>;
    type SerializeTupleVariant = Impossible<String, Error>;
    type SerializeMap = Impossible<String, Error>;
    type SerializeStruct = Impossible<String, Error>;
    type SerializeStructVariant = Impossible<String, Error>;

    fn serialize_str(self, v: &str) -> Result<String, Error> {
        Ok(v.to_string())
    }
    fn serialize_bool(self, v: bool) -> Result<String, Error> {
        Ok(v.to_string())
    }
    fn serialize_i8(self, v: i8) -> Result<String, Error> {
        Ok(v.to_string())
    }
    fn serialize_i16(self, v: i16) -> Result<String, Error> {
        Ok(v.to_string())
    }
    fn serialize_i32(self, v: i32) -> Result<String, Error> {
        Ok(v.to_string())
    }
    fn serialize_i64(self, v: i64) -> Result<String, Error> {
        Ok(v.to_string())
    }
    fn serialize_u8(self, v: u8) -> Result<String, Error> {
        Ok(v.to_string())
    }
    fn serialize_u16(self, v: u16) -> Result<String, Error> {
        Ok(v.to_string())
    }
    fn serialize_u32(self, v: u32) -> Result<String, Error> {
        Ok(v.to_string())
    }
    fn serialize_u64(self, v: u64) -> Result<String, Error> {
        Ok(v.to_string())
    }
    fn serialize_f32(self, v: f32) -> Result<String, Error> {
        Ok(tok_f64(v.into()))
    }
    fn serialize_f64(self, v: f64) -> Result<String, Error> {
        Ok(tok_f64(v))
    }
    fn serialize_char(self, v: char) -> Result<String, Error> {
        Ok(v.to_string())
    }
    fn serialize_bytes(self, _v: &[u8]) -> Result<String, Error> {
        Err(Error::msg("map key is not a string"))
    }
    fn serialize_none(self) -> Result<String, Error> {
        Err(Error::msg("map key is not a string"))
    }
    fn serialize_some<T: ?Sized + Serialize>(self, v: &T) -> Result<String, Error> {
        v.serialize(self)
    }
    fn serialize_unit(self) -> Result<String, Error> {
        Err(Error::msg("map key is not a string"))
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<String, Error> {
        Err(Error::msg("map key is not a string"))
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _idx: u32,
        variant: &'static str,
    ) -> Result<String, Error> {
        Ok(variant.to_string())
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        v: &T,
    ) -> Result<String, Error> {
        v.serialize(self)
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        _idx: u32,
        _variant: &'static str,
        _v: &T,
    ) -> Result<String, Error> {
        Err(Error::msg("map key is not a string"))
    }
    fn serialize_seq(self, _len: Option<usize>) -> Result<Impossible<String, Error>, Error> {
        Err(Error::msg("map key is not a string"))
    }
    fn serialize_tuple(self, _len: usize) -> Result<Impossible<String, Error>, Error> {
        Err(Error::msg("map key is not a string"))
    }
    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Impossible<String, Error>, Error> {
        Err(Error::msg("map key is not a string"))
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _idx: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Impossible<String, Error>, Error> {
        Err(Error::msg("map key is not a string"))
    }
    fn serialize_map(self, _len: Option<usize>) -> Result<Impossible<String, Error>, Error> {
        Err(Error::msg("map key is not a string"))
    }
    fn serialize_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Impossible<String, Error>, Error> {
        Err(Error::msg("map key is not a string"))
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _idx: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Impossible<String, Error>, Error> {
        Err(Error::msg("map key is not a string"))
    }
}
