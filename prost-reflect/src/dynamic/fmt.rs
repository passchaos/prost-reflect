use std::fmt::{self, Display, Formatter, Write};

use prost::Message;

use crate::{
    dynamic::{
        fields::ValueAndDescriptor,
        unknown::{UnknownField, UnknownFieldSet},
    },
    DynamicMessage, Kind, MapKey, Value,
};

use super::SetFieldError;

struct FormatOptions {
    pub pretty: bool,
    pub skip_unknown_fields: bool,
    pub expand_any: bool,
}

impl Display for Value {
    /// Formats this value using the protobuf text format.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::{collections::HashMap, iter::FromIterator};
    /// # use prost_reflect::{MapKey, Value};
    /// assert_eq!(format!("{}", Value::String("hello".to_owned())), "\"hello\"");
    /// assert_eq!(format!("{}", Value::List(vec![Value::I32(1), Value::I32(2)])), "[1,2]");
    /// assert_eq!(format!("{}", Value::Map(HashMap::from_iter([(MapKey::I32(1), Value::U32(2))]))), "[{key:1,value:2}]");
    /// // The alternate format specifier may be used to indent the output
    /// assert_eq!(format!("{:#}", Value::Map(HashMap::from_iter([(MapKey::I32(1), Value::U32(2))]))), "[{\n  key: 1\n  value: 2\n}]");
    /// ```
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Writer::new(FormatOptions::from_formatter(f), f).fmt_value(self, None)
    }
}

impl Display for DynamicMessage {
    /// Formats this message using the protobuf text format.
    ///
    /// # Examples
    ///
    /// ```
    /// # use prost::Message;
    /// # use prost_types::FileDescriptorSet;
    /// # use prost_reflect::{DynamicMessage, DescriptorPool, Value};
    /// # let pool = DescriptorPool::decode(include_bytes!("../file_descriptor_set.bin").as_ref()).unwrap();
    /// # let message_descriptor = pool.get_message_by_name("package.MyMessage").unwrap();
    /// let dynamic_message = DynamicMessage::decode(message_descriptor, b"\x08\x96\x01\x1a\x02\x10\x42".as_ref()).unwrap();
    /// assert_eq!(format!("{}", dynamic_message), "foo:150,nested{bar:66}");
    /// // The alternate format specifier may be used to pretty-print the output
    /// assert_eq!(format!("{:#}", dynamic_message), "foo: 150\nnested {\n  bar: 66\n}");
    /// ```
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Writer::new(FormatOptions::from_formatter(f), f).fmt_message(self)
    }
}

impl Display for UnknownFieldSet {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Writer::new(
            FormatOptions {
                skip_unknown_fields: false,
                ..FormatOptions::from_formatter(f)
            },
            f,
        )
        .fmt_delimited(self.fields(), Writer::fmt_unknown_field)
    }
}

impl Display for SetFieldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SetFieldError::NotFound => write!(f, "field not found"),
            SetFieldError::InvalidType { field, value } => {
                write!(f, "expected a value of type '")?;
                if field.is_map() {
                    let entry = field.kind();
                    let entry = entry.as_message().unwrap();
                    write!(f, "map<{:?}, {:?}>", entry.map_entry_key_field().kind(), entry.map_entry_value_field().kind())?;
                } else if field.is_list() {
                    write!(f, "repeated {:?}", field.kind())?;
                } else {
                    write!(f, "{:?}", field.kind())?;
                }
                write!(f, "', but found '{}'", value)
            },
        }
    }
}

impl FormatOptions {
    fn from_formatter(f: &mut Formatter) -> Self {
        FormatOptions {
            pretty: f.alternate(),
            ..Default::default()
        }
    }
}

impl Default for FormatOptions {
    fn default() -> Self {
        FormatOptions {
            pretty: false,
            skip_unknown_fields: true,
            expand_any: true,
        }
    }
}

struct Writer<'a, W> {
    options: FormatOptions,
    f: &'a mut W,
    indent_level: u32,
}

impl<'a, W> Writer<'a, W>
where
    W: Write,
{
    fn new(options: FormatOptions, f: &'a mut W) -> Self {
        Writer {
            options,
            f,
            indent_level: 0,
        }
    }

    fn fmt_message(&mut self, message: &DynamicMessage) -> fmt::Result {
        if self.options.expand_any {
            if let Some((type_url, body)) = as_any(message) {
                self.f.write_char('[')?;
                self.f.write_str(&type_url)?;
                self.f.write_str("]")?;
                self.fmt_field_value(&Value::Message(body), None)?;
                return Ok(());
            }
        }

        let fields = message.fields.iter(&message.desc);
        if self.options.skip_unknown_fields {
            self.fmt_delimited(
                fields.filter(|f| !matches!(f, ValueAndDescriptor::Unknown(..))),
                Writer::fmt_message_field,
            )
        } else {
            self.fmt_delimited(fields, Writer::fmt_message_field)
        }
    }

    fn fmt_value(&mut self, value: &Value, kind: Option<&Kind>) -> fmt::Result {
        match value {
            Value::Bool(value) => write!(self.f, "{}", value),
            Value::I32(value) => write!(self.f, "{}", value),
            Value::I64(value) => write!(self.f, "{}", value),
            Value::U32(value) => write!(self.f, "{}", value),
            Value::U64(value) => write!(self.f, "{}", value),
            Value::F32(value) => write!(self.f, "{}", value),
            Value::F64(value) => write!(self.f, "{}", value),
            Value::String(s) => self.fmt_string(s.as_bytes()),
            Value::Bytes(s) => self.fmt_string(s.as_ref()),
            Value::EnumNumber(value) => {
                if let Some(Kind::Enum(desc)) = kind {
                    if let Some(value) = desc.get_value(*value) {
                        return self.f.write_str(value.name());
                    }
                }
                write!(self.f, "{}", value)
            }
            Value::Message(message) => {
                if message.fields.iter(&message.desc).all(|f| {
                    self.options.skip_unknown_fields && matches!(f, ValueAndDescriptor::Unknown(..))
                }) {
                    self.f.write_str("{}")
                } else if self.options.pretty {
                    self.f.write_char('{')?;
                    self.indent_level += 2;
                    self.fmt_newline()?;
                    self.fmt_message(message)?;
                    self.indent_level -= 2;
                    self.fmt_newline()?;
                    self.f.write_char('}')
                } else {
                    self.f.write_char('{')?;
                    self.fmt_message(message)?;
                    self.f.write_char('}')
                }
            }
            Value::List(list) => {
                self.fmt_list(list.iter(), |this, value| this.fmt_value(value, kind))
            }
            Value::Map(map) => {
                let value_kind = kind
                    .and_then(|k| k.as_message())
                    .map(|m| m.map_entry_value_field().kind());
                self.fmt_list(map.iter(), |this, (key, value)| {
                    if this.options.pretty {
                        this.f.write_str("{")?;
                        this.indent_level += 2;
                        this.fmt_newline()?;
                        this.f.write_str("key: ")?;
                        this.fmt_map_key(key)?;
                        this.fmt_newline()?;
                        this.f.write_str("value")?;
                        this.fmt_field_value(value, value_kind.as_ref())?;
                        this.indent_level -= 2;
                        this.fmt_newline()?;
                        this.f.write_char('}')
                    } else {
                        this.f.write_str("{key:")?;
                        this.fmt_map_key(key)?;
                        this.f.write_str(",value")?;
                        this.fmt_field_value(value, value_kind.as_ref())?;
                        this.f.write_char('}')
                    }
                })
            }
        }
    }

    fn fmt_map_key(&mut self, value: &MapKey) -> fmt::Result {
        match value {
            MapKey::Bool(value) => write!(self.f, "{}", value),
            MapKey::I32(value) => write!(self.f, "{}", value),
            MapKey::I64(value) => write!(self.f, "{}", value),
            MapKey::U32(value) => write!(self.f, "{}", value),
            MapKey::U64(value) => write!(self.f, "{}", value),
            MapKey::String(s) => self.fmt_string(s.as_bytes()),
        }
    }

    fn fmt_message_field(&mut self, field: ValueAndDescriptor) -> fmt::Result {
        match field {
            ValueAndDescriptor::Field(value, desc) => {
                if desc.is_group() {
                    write!(self.f, "{}", desc.kind().as_message().unwrap().name())?;
                } else {
                    write!(self.f, "{}", desc.name())?;
                }
                self.fmt_field_value(&value, Some(&desc.kind()))
            }
            ValueAndDescriptor::Extension(value, desc) => {
                if desc.is_group() {
                    write!(
                        self.f,
                        "[{}]",
                        desc.kind().as_message().unwrap().full_name()
                    )?;
                } else {
                    write!(self.f, "[{}]", desc.full_name())?;
                }
                self.fmt_field_value(&value, Some(&desc.kind()))
            }
            ValueAndDescriptor::Unknown(number, values) => self.fmt_delimited(
                values.iter().map(|value| (number, value)),
                Writer::fmt_unknown_field,
            ),
        }
    }

    fn fmt_field_value(&mut self, value: &Value, kind: Option<&Kind>) -> fmt::Result {
        if !matches!(value, Value::Message(_)) {
            self.f.write_char(':')?;
        }
        self.fmt_padding()?;
        self.fmt_value(value, kind)
    }

    fn fmt_unknown_field(&mut self, (number, value): (u32, &UnknownField)) -> fmt::Result {
        write!(self.f, "{}", number)?;
        match value {
            UnknownField::Varint(int) => {
                self.f.write_char(':')?;
                self.fmt_padding()?;
                write!(self.f, "{}", int)
            }
            UnknownField::ThirtyTwoBit(bytes) => {
                self.f.write_char(':')?;
                self.fmt_padding()?;
                write!(self.f, "0x{:08x}", u32::from_le_bytes(*bytes))
            }
            UnknownField::SixtyFourBit(bytes) => {
                self.f.write_char(':')?;
                self.fmt_padding()?;
                write!(self.f, "0x{:016x}", u64::from_le_bytes(*bytes))
            }
            UnknownField::LengthDelimited(bytes) => {
                if !bytes.is_empty() {
                    if let Ok(set) = UnknownFieldSet::decode(bytes.clone()) {
                        self.fmt_padding()?;
                        return self.fmt_unknown_field_set(&set);
                    }
                }

                self.f.write_char(':')?;
                self.fmt_padding()?;
                self.fmt_string(bytes.as_ref())
            }
            UnknownField::Group(set) => {
                self.fmt_padding()?;
                self.fmt_unknown_field_set(set)
            }
        }
    }

    fn fmt_unknown_field_set(&mut self, set: &UnknownFieldSet) -> fmt::Result {
        if set.is_empty() {
            self.f.write_str("{}")
        } else if self.options.pretty {
            self.f.write_char('{')?;
            self.indent_level += 2;
            self.fmt_newline()?;
            self.fmt_delimited(set.fields(), Writer::fmt_unknown_field)?;
            self.indent_level -= 2;
            self.fmt_newline()?;
            self.f.write_char('}')
        } else {
            self.f.write_char('{')?;
            self.fmt_delimited(set.fields(), Writer::fmt_unknown_field)?;
            self.f.write_char('}')
        }
    }

    fn fmt_string(&mut self, bytes: &[u8]) -> fmt::Result {
        self.f.write_char('"')?;
        for &ch in bytes {
            match ch {
                b'\t' => self.f.write_str("\\t")?,
                b'\r' => self.f.write_str("\\r")?,
                b'\n' => self.f.write_str("\\n")?,
                b'\\' => self.f.write_str("\\\\")?,
                b'\'' => self.f.write_str("\\'")?,
                b'"' => self.f.write_str("\\\"")?,
                b'\x20'..=b'\x7e' => self.f.write_char(ch as char)?,
                _ => {
                    write!(self.f, "\\{:03o}", ch)?;
                }
            }
        }
        self.f.write_char('"')
    }

    fn fmt_delimited<T>(
        &mut self,
        mut iter: impl Iterator<Item = T>,
        f: impl Fn(&mut Self, T) -> fmt::Result,
    ) -> fmt::Result {
        if let Some(first) = iter.next() {
            f(self, first)?;
        }
        for item in iter {
            if self.options.pretty {
                self.fmt_newline()?;
            } else {
                self.f.write_char(',')?;
            }
            f(self, item)?;
        }

        Ok(())
    }

    fn fmt_list<I>(
        &mut self,
        mut iter: impl Iterator<Item = I>,
        f: impl Fn(&mut Self, I) -> fmt::Result,
    ) -> fmt::Result {
        self.f.write_char('[')?;
        if let Some(first) = iter.next() {
            f(self, first)?;
        }
        for item in iter {
            self.f.write_char(',')?;
            self.fmt_padding()?;
            f(self, item)?;
        }
        self.f.write_char(']')
    }

    fn fmt_padding(&mut self) -> fmt::Result {
        if self.options.pretty {
            self.f.write_char(' ')?;
        }
        Ok(())
    }

    fn fmt_newline(&mut self) -> fmt::Result {
        self.f.write_char('\n')?;
        for _ in 0..self.indent_level {
            self.f.write_char(' ')?;
        }
        Ok(())
    }
}

fn as_any(message: &DynamicMessage) -> Option<(String, DynamicMessage)> {
    if message.desc.full_name() != "google.protobuf.Any" {
        return None;
    }

    let any = message.transcode_to::<prost_types::Any>().ok()?;
    let message_name = any
        .type_url
        .strip_prefix("type.googleapis.com/")
        .or_else(|| any.type_url.strip_prefix("type.googleprod.com/"))?;

    let desc = message
        .desc
        .parent_pool()
        .get_message_by_name(message_name)?;
    let body = DynamicMessage::decode(desc, any.value.as_slice()).ok()?;
    Some((any.type_url, body))
}

#[test]
fn fmt_unknown_scalar() {
    let value = UnknownFieldSet::decode(b"\x09\x9a\x99\x99\x99\x99\x99\xf1\x3f\x15\xcd\xcc\x0c\x40\x18\x03\x20\x04\x28\x05\x30\x06\x38\x0e\x40\x10\x4d\x09\x00\x00\x00\x51\x0a\x00\x00\x00\x00\x00\x00\x00\x5d\x0b\x00\x00\x00\x61\x0c\x00\x00\x00\x00\x00\x00\x00\x68\x01\x72\x01\x35\x7a\x07\x69\xa6\xbe\x6d\xb6\xff\x58".as_ref()).unwrap();
    assert_eq!(
        format!("{}", value),
        r#"1:0x3ff199999999999a,2:0x400ccccd,3:3,4:4,5:5,6:6,7:14,8:16,9:0x00000009,10:0x000000000000000a,11:0x0000000b,12:0x000000000000000c,13:1,14:"5",15:"i\246\276m\266\377X""#
    );
    assert_eq!(
        format!("{:#}", value),
        r#"1: 0x3ff199999999999a
2: 0x400ccccd
3: 3
4: 4
5: 5
6: 6
7: 14
8: 16
9: 0x00000009
10: 0x000000000000000a
11: 0x0000000b
12: 0x000000000000000c
13: 1
14: "5"
15: "i\246\276m\266\377X""#
    );
}

#[test]
fn fmt_unknown_complex_type() {
    let value = UnknownFieldSet::decode(b"\x0a\x15\x0a\x01\x31\x12\x10\x09\x9a\x99\x99\x99\x99\x99\xf1\x3f\x15\xcd\xcc\x0c\x40\x18\x03\x12\x0d\x08\x03\x12\x09\x38\x0e\x40\x10\x4d\x09\x00\x00\x00\x1a\x16\x5d\x0b\x00\x00\x00\x61\x0c\x00\x00\x00\x00\x00\x00\x00\x68\x01\x72\x01\x35\x7a\x01\x36\x22\x0e\x00\x01\x02\x03\xfc\xff\xff\xff\xff\xff\xff\xff\xff\x01\x28\x01".as_ref()).unwrap();
    assert_eq!(
        format!("{}", value),
        r#"1{1:"1",2{1:0x3ff199999999999a,2:0x400ccccd,3:3}},2{1:3,2{7:14,8:16,9:0x00000009}},3{11:0x0000000b,12:0x000000000000000c,13:1,14:"5",15:"6"},4:"\000\001\002\003\374\377\377\377\377\377\377\377\377\001",5:1"#
    );
    assert_eq!(
        format!("{:#}", value),
        r#"1 {
  1: "1"
  2 {
    1: 0x3ff199999999999a
    2: 0x400ccccd
    3: 3
  }
}
2 {
  1: 3
  2 {
    7: 14
    8: 16
    9: 0x00000009
  }
}
3 {
  11: 0x0000000b
  12: 0x000000000000000c
  13: 1
  14: "5"
  15: "6"
}
4: "\000\001\002\003\374\377\377\377\377\377\377\377\377\001"
5: 1"#
    );
}

#[test]
fn fmt_unknown_group() {
    let value = UnknownFieldSet::decode(b"\x0b\x0a\x03\x62\x61\x72\x0c\x13\x0a\x03\x66\x6f\x6f\x10\xfb\xff\xff\xff\xff\xff\xff\xff\xff\x01\x14\x1b\x0a\x00\x1c\x1b\x0a\x05\x68\x65\x6c\x6c\x6f\x10\x0a\x1c".as_ref()).unwrap();
    assert_eq!(
        format!("{}", value),
        r#"1{1:"bar"},2{1:"foo",2:18446744073709551611},3{1:""},3{1:"hello",2:10}"#
    );
    assert_eq!(
        format!("{:#}", value),
        r#"1 {
  1: "bar"
}
2 {
  1: "foo"
  2: 18446744073709551611
}
3 {
  1: ""
}
3 {
  1: "hello"
  2: 10
}"#
    );
}
