//! Compact per-layout index summaries.

use kronika_format::BlobEntry;
use kronika_registry::{Column, ColumnClass, ColumnType, MAX_SECTION_ROWS, TypeContract, contract};

use crate::build::DERIVED_HEALTH_TYPE_ID;
use crate::file::IndexError;

const MAX_ITEMS: usize = 1_000_000;
const MAX_VALUE_BYTES: usize = 1024 * 1024;

/// One exact, resolved identity value.
#[derive(Debug, Clone, PartialEq)]
pub enum IdentityValue {
    /// A nullable identity cell with no value.
    Null,
    /// Signed 16-bit registry value.
    I16(i16),
    /// Signed 32-bit registry value.
    I32(i32),
    /// Signed 64-bit registry value.
    I64(i64),
    /// Unsigned 32-bit registry value.
    U32(u32),
    /// Unsigned 64-bit registry value.
    U64(u64),
    /// Floating-point registry value, preserved by its IEEE bits.
    F64(f64),
    /// Boolean registry value.
    Bool(bool),
    /// Timestamp identity, unix microseconds.
    Ts(i64),
    /// Exact bytes resolved from `dict.strings`.
    Text(Vec<u8>),
    /// Exact stored blob representation and metadata.
    Blob {
        /// Stored bytes, possibly a prefix of the original.
        stored_bytes: Vec<u8>,
        /// Original byte length.
        full_len: u64,
        /// Whether only a prefix is stored.
        truncated: bool,
        /// SHA-256 of the original when truncated.
        full_sha256: Option<[u8; 32]>,
    },
    /// Exact `List<i32>` identity value.
    ListI32(Vec<i32>),
}

impl IdentityValue {
    pub(crate) fn from_blob(blob: BlobEntry<'_>) -> Self {
        Self::Blob {
            stored_bytes: blob.stored_bytes.to_vec(),
            full_len: blob.full_len,
            truncated: blob.truncated,
            full_sha256: blob.full_sha256,
        }
    }
}

/// One exact numeric value in a layout summary.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Number {
    /// Signed 16-bit value.
    I16(i16),
    /// Signed 32-bit value.
    I32(i32),
    /// Signed 64-bit value.
    I64(i64),
    /// Unsigned 32-bit value.
    U32(u32),
    /// Unsigned 64-bit value.
    U64(u64),
    /// Floating-point value, preserved by its IEEE bits.
    F64(f64),
}

/// One timestamped endpoint of an observed numeric series.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    /// Unix-microsecond timestamp.
    pub ts: i64,
    /// Exact stored numeric type and value.
    pub value: Number,
}

/// Bounded facts about one numeric series inside one segment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Observation {
    /// Number of non-null numeric readings.
    pub count: u64,
    /// Earliest reading, absent when `count` is zero.
    pub first: Option<Sample>,
    /// Latest reading, absent when `count` is zero.
    pub last: Option<Sample>,
    /// Nonnegative last-minus-first difference for a cumulative column.
    pub nonnegative_delta: Option<Number>,
    /// Time from the first to the last reading in microseconds.
    pub observed_us: u64,
}

/// One exact identity and its numeric summaries.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectSummary {
    /// Values in the contract's declared identity order.
    pub identity: Vec<IdentityValue>,
    /// Cumulative and gauge columns in physical contract order.
    pub observations: Vec<Observation>,
}

/// Independently decodable block for one physical registry layout.
#[derive(Debug, Clone, PartialEq)]
pub struct SectionSummary {
    /// Exact physical registry `type_id`.
    pub type_id: u32,
    /// Objects remain separate within this physical layout.
    pub objects: Vec<ObjectSummary>,
}

pub(crate) fn encode_section(section: &SectionSummary) -> Result<Vec<u8>, IndexError> {
    validate_section(section)?;
    let mut out = Vec::new();
    put_u32(&mut out, section.type_id);
    put_len(&mut out, section.objects.len())?;
    for object in &section.objects {
        put_len(&mut out, object.identity.len())?;
        for value in &object.identity {
            encode_identity(&mut out, value)?;
        }
        put_len(&mut out, object.observations.len())?;
        for observation in &object.observations {
            encode_observation(&mut out, *observation);
        }
    }
    Ok(out)
}

pub(crate) fn decode_section(bytes: &[u8], expected: u32) -> Result<SectionSummary, IndexError> {
    let mut input = Input::new(bytes);
    let type_id = input.u32()?;
    if type_id != expected {
        return Err(IndexError::BadLayout);
    }
    let shape = Shape::of(type_id)?;
    let object_count = input.len()?;
    if object_count > MAX_SECTION_ROWS {
        return Err(IndexError::BadLayout);
    }
    let mut objects = Vec::with_capacity(object_count);
    for _ in 0..object_count {
        let identity_count = input.len()?;
        if identity_count != shape.identity_count {
            return Err(IndexError::BadLayout);
        }
        let mut identity = Vec::with_capacity(identity_count);
        for _ in 0..identity_count {
            identity.push(input.identity()?);
        }
        let observation_count = input.len()?;
        if observation_count != shape.observation_count {
            return Err(IndexError::BadLayout);
        }
        let mut observations = Vec::with_capacity(observation_count);
        for _ in 0..observation_count {
            observations.push(input.observation()?);
        }
        objects.push(ObjectSummary {
            identity,
            observations,
        });
    }
    input.finish()?;
    let section = SectionSummary { type_id, objects };
    validate_section(&section)?;
    Ok(section)
}

#[derive(Debug, Clone, Copy)]
struct Shape {
    contract: Option<&'static TypeContract>,
    identity_count: usize,
    observation_count: usize,
}

impl Shape {
    fn of(type_id: u32) -> Result<Self, IndexError> {
        if type_id == DERIVED_HEALTH_TYPE_ID {
            return Ok(Self {
                contract: None,
                identity_count: 0,
                observation_count: 1,
            });
        }
        let contract = contract(type_id).ok_or(IndexError::BadLayout)?;
        Ok(Self {
            contract: Some(contract),
            identity_count: contract.identity.len(),
            observation_count: numeric_columns(contract).count(),
        })
    }
}

pub(crate) fn validate_section(section: &SectionSummary) -> Result<(), IndexError> {
    let shape = Shape::of(section.type_id)?;
    if section.objects.len() > MAX_SECTION_ROWS {
        return Err(IndexError::BadLayout);
    }
    if section.type_id == DERIVED_HEALTH_TYPE_ID {
        if section.objects.len() != 1 {
            return Err(IndexError::BadLayout);
        }
        let object = &section.objects[0];
        if !object.identity.is_empty() || object.observations.len() != 1 {
            return Err(IndexError::BadLayout);
        }
        return validate_observation(&object.observations[0], ColumnClass::Gauge, ColumnType::U32);
    }

    let contract = shape.contract.ok_or(IndexError::BadLayout)?;
    let identity_columns: Vec<&Column> = contract
        .identity
        .iter()
        .map(|name| contract.column(name).ok_or(IndexError::BadLayout))
        .collect::<Result<_, _>>()?;
    let numeric_columns: Vec<&Column> = numeric_columns(contract).collect();
    for object in &section.objects {
        if object.identity.len() != identity_columns.len()
            || object.observations.len() != numeric_columns.len()
        {
            return Err(IndexError::BadLayout);
        }
        for (value, column) in object.identity.iter().zip(&identity_columns) {
            validate_identity(value, column)?;
        }
        for (observation, column) in object.observations.iter().zip(&numeric_columns) {
            validate_observation(observation, column.class, column.ty)?;
        }
    }
    Ok(())
}

fn numeric_columns(contract: &TypeContract) -> impl Iterator<Item = &Column> {
    contract
        .columns
        .iter()
        .filter(|column| matches!(column.class, ColumnClass::Cumulative | ColumnClass::Gauge))
}

fn validate_identity(value: &IdentityValue, column: &Column) -> Result<(), IndexError> {
    let valid = match value {
        IdentityValue::Null => column.nullable,
        IdentityValue::I16(_) => matches!(column.ty, ColumnType::I8 | ColumnType::I16),
        IdentityValue::I32(_) => column.ty == ColumnType::I32,
        IdentityValue::I64(_) => column.ty == ColumnType::I64,
        IdentityValue::U32(_) => {
            matches!(
                column.ty,
                ColumnType::U8 | ColumnType::U16 | ColumnType::U32
            )
        }
        IdentityValue::U64(_) => column.ty == ColumnType::U64,
        IdentityValue::F64(_) => matches!(column.ty, ColumnType::F32 | ColumnType::F64),
        IdentityValue::Bool(_) => column.ty == ColumnType::Bool,
        IdentityValue::Ts(_) => column.ty == ColumnType::Ts,
        IdentityValue::Text(_) => column.ty == ColumnType::StrId,
        IdentityValue::Blob {
            stored_bytes,
            full_len,
            truncated,
            full_sha256,
        } => {
            let stored_len = u64::try_from(stored_bytes.len()).unwrap_or(u64::MAX);
            column.ty == ColumnType::StrId
                && stored_len <= *full_len
                && if *truncated {
                    stored_len < *full_len && full_sha256.is_some()
                } else {
                    stored_len == *full_len && full_sha256.is_none()
                }
        }
        IdentityValue::ListI32(_) => column.ty == ColumnType::ListI32,
    };
    if valid {
        Ok(())
    } else {
        Err(IndexError::BadLayout)
    }
}

fn validate_observation(
    observation: &Observation,
    class: ColumnClass,
    ty: ColumnType,
) -> Result<(), IndexError> {
    let endpoints = match (observation.first, observation.last) {
        (None, None) if observation.count == 0 => None,
        (Some(first), Some(last)) if observation.count != 0 => {
            if first.ts > last.ts
                || !number_matches(first.value, ty)
                || !number_matches(last.value, ty)
            {
                return Err(IndexError::BadLayout);
            }
            let observed_us = u64::try_from(i128::from(last.ts) - i128::from(first.ts))
                .map_err(|_overflow| IndexError::BadLayout)?;
            if observation.observed_us != observed_us {
                return Err(IndexError::BadLayout);
            }
            Some((first.value, last.value))
        }
        _ => return Err(IndexError::BadLayout),
    };

    if let Some(delta) = observation.nonnegative_delta
        && !number_matches(delta, ty)
    {
        return Err(IndexError::BadLayout);
    }
    let expected_delta = if class == ColumnClass::Cumulative && observation.count >= 2 {
        endpoints.and_then(|(first, last)| nonnegative_difference(first, last))
    } else {
        None
    };
    if !same_number(observation.nonnegative_delta, expected_delta) {
        return Err(IndexError::BadLayout);
    }
    Ok(())
}

fn number_matches(number: Number, ty: ColumnType) -> bool {
    match number {
        Number::I16(_) => matches!(ty, ColumnType::I8 | ColumnType::I16),
        Number::I32(_) => ty == ColumnType::I32,
        Number::I64(_) => matches!(ty, ColumnType::I64 | ColumnType::Ts),
        Number::U32(_) => matches!(ty, ColumnType::U8 | ColumnType::U16 | ColumnType::U32),
        Number::U64(_) => ty == ColumnType::U64,
        Number::F64(_) => matches!(ty, ColumnType::F32 | ColumnType::F64),
    }
}

fn nonnegative_difference(before: Number, after: Number) -> Option<Number> {
    match (before, after) {
        (Number::I16(before), Number::I16(after)) if after >= before => {
            after.checked_sub(before).map(Number::I16)
        }
        (Number::I32(before), Number::I32(after)) if after >= before => {
            after.checked_sub(before).map(Number::I32)
        }
        (Number::I64(before), Number::I64(after)) if after >= before => {
            after.checked_sub(before).map(Number::I64)
        }
        (Number::U32(before), Number::U32(after)) => after.checked_sub(before).map(Number::U32),
        (Number::U64(before), Number::U64(after)) => after.checked_sub(before).map(Number::U64),
        (Number::F64(before), Number::F64(after)) if after >= before => {
            let delta = after - before;
            delta.is_finite().then_some(Number::F64(delta))
        }
        _ => None,
    }
}

const fn same_number(left: Option<Number>, right: Option<Number>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(Number::I16(left)), Some(Number::I16(right))) => left == right,
        (Some(Number::I32(left)), Some(Number::I32(right))) => left == right,
        (Some(Number::I64(left)), Some(Number::I64(right))) => left == right,
        (Some(Number::U32(left)), Some(Number::U32(right))) => left == right,
        (Some(Number::U64(left)), Some(Number::U64(right))) => left == right,
        (Some(Number::F64(left)), Some(Number::F64(right))) => left.to_bits() == right.to_bits(),
        _ => false,
    }
}

fn encode_identity(out: &mut Vec<u8>, value: &IdentityValue) -> Result<(), IndexError> {
    match value {
        IdentityValue::Null => out.push(0),
        IdentityValue::I16(value) => {
            out.push(1);
            out.extend_from_slice(&value.to_le_bytes());
        }
        IdentityValue::I32(value) => {
            out.push(2);
            out.extend_from_slice(&value.to_le_bytes());
        }
        IdentityValue::I64(value) => {
            out.push(3);
            put_i64(out, *value);
        }
        IdentityValue::U32(value) => {
            out.push(4);
            put_u32(out, *value);
        }
        IdentityValue::U64(value) => {
            out.push(5);
            put_u64(out, *value);
        }
        IdentityValue::F64(value) => {
            out.push(6);
            put_u64(out, value.to_bits());
        }
        IdentityValue::Bool(value) => {
            out.push(7);
            out.push(u8::from(*value));
        }
        IdentityValue::Ts(value) => {
            out.push(8);
            put_i64(out, *value);
        }
        IdentityValue::Text(bytes) => {
            out.push(9);
            put_bytes(out, bytes)?;
        }
        IdentityValue::Blob {
            stored_bytes,
            full_len,
            truncated,
            full_sha256,
        } => {
            out.push(10);
            put_bytes(out, stored_bytes)?;
            put_u64(out, *full_len);
            out.push(u8::from(*truncated));
            match full_sha256 {
                Some(hash) => {
                    out.push(1);
                    out.extend_from_slice(hash);
                }
                None => out.push(0),
            }
        }
        IdentityValue::ListI32(values) => {
            out.push(11);
            put_len(out, values.len())?;
            for value in values {
                out.extend_from_slice(&value.to_le_bytes());
            }
        }
    }
    Ok(())
}

fn encode_observation(out: &mut Vec<u8>, observation: Observation) {
    put_u64(out, observation.count);
    put_sample(out, observation.first);
    put_sample(out, observation.last);
    put_number(out, observation.nonnegative_delta);
    put_u64(out, observation.observed_us);
}

fn put_sample(out: &mut Vec<u8>, sample: Option<Sample>) {
    match sample {
        Some(sample) => {
            out.push(1);
            put_i64(out, sample.ts);
            put_number(out, Some(sample.value));
        }
        None => out.push(0),
    }
}

fn put_number(out: &mut Vec<u8>, number: Option<Number>) {
    match number {
        None => out.push(0),
        Some(Number::I16(value)) => {
            out.push(1);
            out.extend_from_slice(&value.to_le_bytes());
        }
        Some(Number::I32(value)) => {
            out.push(2);
            out.extend_from_slice(&value.to_le_bytes());
        }
        Some(Number::I64(value)) => {
            out.push(3);
            put_i64(out, value);
        }
        Some(Number::U32(value)) => {
            out.push(4);
            put_u32(out, value);
        }
        Some(Number::U64(value)) => {
            out.push(5);
            put_u64(out, value);
        }
        Some(Number::F64(value)) => {
            out.push(6);
            put_u64(out, value.to_bits());
        }
    }
}

fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), IndexError> {
    if bytes.len() > MAX_VALUE_BYTES {
        return Err(IndexError::BadLayout);
    }
    put_len(out, bytes.len())?;
    out.extend_from_slice(bytes);
    Ok(())
}

fn put_len(out: &mut Vec<u8>, len: usize) -> Result<(), IndexError> {
    if len > MAX_ITEMS {
        return Err(IndexError::BadLayout);
    }
    let len = u32::try_from(len).map_err(|_overflow| IndexError::BadLayout)?;
    put_u32(out, len);
    Ok(())
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_i64(out: &mut Vec<u8>, value: i64) {
    out.extend_from_slice(&value.to_le_bytes());
}

struct Input<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Input<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], IndexError> {
        let end = self.at.checked_add(len).ok_or(IndexError::Truncated)?;
        let value = self.bytes.get(self.at..end).ok_or(IndexError::Truncated)?;
        self.at = end;
        Ok(value)
    }

    fn u8(&mut self) -> Result<u8, IndexError> {
        Ok(*self.take(1)?.first().ok_or(IndexError::Truncated)?)
    }

    fn u32(&mut self) -> Result<u32, IndexError> {
        let raw: [u8; 4] = self
            .take(4)?
            .try_into()
            .map_err(|_error| IndexError::Truncated)?;
        Ok(u32::from_le_bytes(raw))
    }

    fn i32(&mut self) -> Result<i32, IndexError> {
        let raw: [u8; 4] = self
            .take(4)?
            .try_into()
            .map_err(|_error| IndexError::Truncated)?;
        Ok(i32::from_le_bytes(raw))
    }

    fn u64(&mut self) -> Result<u64, IndexError> {
        let raw: [u8; 8] = self
            .take(8)?
            .try_into()
            .map_err(|_error| IndexError::Truncated)?;
        Ok(u64::from_le_bytes(raw))
    }

    fn i64(&mut self) -> Result<i64, IndexError> {
        let raw: [u8; 8] = self
            .take(8)?
            .try_into()
            .map_err(|_error| IndexError::Truncated)?;
        Ok(i64::from_le_bytes(raw))
    }

    fn len(&mut self) -> Result<usize, IndexError> {
        let len = self.u32()? as usize;
        if len > MAX_ITEMS {
            return Err(IndexError::BadLayout);
        }
        Ok(len)
    }

    fn bytes(&mut self) -> Result<Vec<u8>, IndexError> {
        let len = self.len()?;
        if len > MAX_VALUE_BYTES {
            return Err(IndexError::BadLayout);
        }
        Ok(self.take(len)?.to_vec())
    }

    fn identity(&mut self) -> Result<IdentityValue, IndexError> {
        match self.u8()? {
            0 => Ok(IdentityValue::Null),
            1 => Ok(IdentityValue::I16(i16::from_le_bytes(
                self.take(2)?
                    .try_into()
                    .map_err(|_error| IndexError::Truncated)?,
            ))),
            2 => Ok(IdentityValue::I32(self.i32()?)),
            3 => Ok(IdentityValue::I64(self.i64()?)),
            4 => Ok(IdentityValue::U32(self.u32()?)),
            5 => Ok(IdentityValue::U64(self.u64()?)),
            6 => Ok(IdentityValue::F64(f64::from_bits(self.u64()?))),
            7 => match self.u8()? {
                0 => Ok(IdentityValue::Bool(false)),
                1 => Ok(IdentityValue::Bool(true)),
                _ => Err(IndexError::BadLayout),
            },
            8 => Ok(IdentityValue::Ts(self.i64()?)),
            9 => Ok(IdentityValue::Text(self.bytes()?)),
            10 => {
                let stored_bytes = self.bytes()?;
                let full_len = self.u64()?;
                let truncated = match self.u8()? {
                    0 => false,
                    1 => true,
                    _ => return Err(IndexError::BadLayout),
                };
                let full_sha256 = match self.u8()? {
                    0 => None,
                    1 => Some(
                        self.take(32)?
                            .try_into()
                            .map_err(|_error| IndexError::Truncated)?,
                    ),
                    _ => return Err(IndexError::BadLayout),
                };
                Ok(IdentityValue::Blob {
                    stored_bytes,
                    full_len,
                    truncated,
                    full_sha256,
                })
            }
            11 => {
                let count = self.len()?;
                let mut values = Vec::with_capacity(count);
                for _ in 0..count {
                    values.push(self.i32()?);
                }
                Ok(IdentityValue::ListI32(values))
            }
            _ => Err(IndexError::BadLayout),
        }
    }

    fn number(&mut self) -> Result<Option<Number>, IndexError> {
        match self.u8()? {
            0 => Ok(None),
            1 => Ok(Some(Number::I16(i16::from_le_bytes(
                self.take(2)?
                    .try_into()
                    .map_err(|_error| IndexError::Truncated)?,
            )))),
            2 => Ok(Some(Number::I32(self.i32()?))),
            3 => Ok(Some(Number::I64(self.i64()?))),
            4 => Ok(Some(Number::U32(self.u32()?))),
            5 => Ok(Some(Number::U64(self.u64()?))),
            6 => Ok(Some(Number::F64(f64::from_bits(self.u64()?)))),
            _ => Err(IndexError::BadLayout),
        }
    }

    fn sample(&mut self) -> Result<Option<Sample>, IndexError> {
        match self.u8()? {
            0 => Ok(None),
            1 => {
                let ts = self.i64()?;
                let value = self.number()?.ok_or(IndexError::BadLayout)?;
                Ok(Some(Sample { ts, value }))
            }
            _ => Err(IndexError::BadLayout),
        }
    }

    fn observation(&mut self) -> Result<Observation, IndexError> {
        let count = self.u64()?;
        let first = self.sample()?;
        let last = self.sample()?;
        let nonnegative_delta = self.number()?;
        let observed_us = self.u64()?;
        if (count == 0) != first.is_none() || (count == 0) != last.is_none() {
            return Err(IndexError::BadLayout);
        }
        Ok(Observation {
            count,
            first,
            last,
            nonnegative_delta,
            observed_us,
        })
    }

    const fn finish(self) -> Result<(), IndexError> {
        if self.at == self.bytes.len() {
            Ok(())
        } else {
            Err(IndexError::BadLayout)
        }
    }
}

#[cfg(test)]
mod tests;
