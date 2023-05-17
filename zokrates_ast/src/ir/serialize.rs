use crate::ir::{check::UnconstrainedVariableDetector, solver_indexer::SolverIndexer};

use super::{ProgIterator, Statement};
use crate::ir::ModuleMap;
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use serde::Deserialize;
use serde_cbor::{self, StreamDeserializer};
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use zokrates_field::*;

type DynamicError = Box<dyn std::error::Error>;

const ZOKRATES_MAGIC: &[u8; 4] = &[0x5a, 0x4f, 0x4b, 0];
const FILE_VERSION: &[u8; 4] = &[3, 0, 0, 0];

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
#[repr(u32)]
pub enum SectionType {
    Parameters = 1,
    Constraints = 2,
    Solvers = 3,
    Modules = 4,
}

impl TryFrom<u32> for SectionType {
    type Error = String;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(SectionType::Parameters),
            2 => Ok(SectionType::Constraints),
            3 => Ok(SectionType::Solvers),
            4 => Ok(SectionType::Modules),
            _ => Err("invalid section type".to_string()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Section {
    pub ty: SectionType,
    pub offset: u64,
    pub length: u64,
}

impl Section {
    pub fn new(ty: SectionType) -> Self {
        Self {
            ty,
            offset: 0,
            length: 0,
        }
    }

    pub fn set_offset(&mut self, offset: u64) {
        self.offset = offset;
    }

    pub fn set_length(&mut self, length: u64) {
        self.length = length;
    }
}

#[derive(Debug, Clone)]
pub struct ProgHeader {
    pub curve_id: [u8; 4],
    pub constraint_count: u32,
    pub return_count: u32,
    pub sections: [Section; 4],
}

impl ProgHeader {
    pub fn curve_name(&self) -> &'static str {
        id_to_name(self.curve_id)
    }

    pub fn write<W: Write>(&self, mut w: W) -> std::io::Result<()> {
        w.write_all(&self.curve_id)?;
        w.write_u32::<LittleEndian>(self.constraint_count)?;
        w.write_u32::<LittleEndian>(self.return_count)?;

        for s in &self.sections {
            w.write_u32::<LittleEndian>(s.ty as u32)?;
            w.write_u64::<LittleEndian>(s.offset)?;
            w.write_u64::<LittleEndian>(s.length)?;
        }

        Ok(())
    }

    pub fn read<R: Read>(r: &mut R) -> std::io::Result<Self> {
        let mut magic = [0; 4];
        r.read_exact(&mut magic)?;

        // Check the magic number, `ZOK`
        if &magic != ZOKRATES_MAGIC {
            return Err(std::io::Error::new(
                ErrorKind::InvalidData,
                "Invalid magic number".to_string(),
            ));
        }

        let mut version = [0; 4];
        r.read_exact(&mut version)?;

        // Check the file version
        if &version != FILE_VERSION {
            return Err(std::io::Error::new(
                ErrorKind::InvalidData,
                "Invalid file version".to_string(),
            ));
        }

        let mut curve_id = [0; 4];
        r.read_exact(&mut curve_id)?;

        let constraint_count = r.read_u32::<LittleEndian>()?;
        let return_count = r.read_u32::<LittleEndian>()?;

        let parameters = Self::read_section(r.by_ref())?;
        let constraints = Self::read_section(r.by_ref())?;
        let solvers = Self::read_section(r.by_ref())?;
        let module_map = Self::read_section(r.by_ref())?;

        Ok(ProgHeader {
            curve_id,
            constraint_count,
            return_count,
            sections: [parameters, constraints, solvers, module_map],
        })
    }

    fn read_section<R: Read>(mut r: R) -> std::io::Result<Section> {
        let id = r.read_u32::<LittleEndian>()?;
        let mut section = Section::new(
            SectionType::try_from(id)
                .map_err(|e| std::io::Error::new(ErrorKind::InvalidData, e))?,
        );
        section.set_offset(r.read_u64::<LittleEndian>()?);
        section.set_length(r.read_u64::<LittleEndian>()?);
        Ok(section)
    }
}

impl<'ast, T: Field, I: IntoIterator<Item = Statement<'ast, T>>> ProgIterator<'ast, T, I> {
    /// serialize a program iterator, returning the number of constraints serialized
    /// Note that we only return constraints, not other statements such as directives
    pub fn serialize<W: Write + Seek>(self, mut w: W) -> Result<usize, DynamicError> {
        use super::folder::Folder;

        w.write_all(&*ZOKRATES_MAGIC)?;
        w.write_all(&*FILE_VERSION)?;

        let header_start = w.stream_position()?;

        // reserve bytes for the header
        w.write_all(&[0u8; std::mem::size_of::<ProgHeader>()])?;

        // write parameters section
        let parameters = {
            let mut section = Section::new(SectionType::Parameters);
            section.set_offset(w.stream_position()?);

            serde_cbor::to_writer(&mut w, &self.arguments)?;

            section.set_length(w.stream_position()? - section.offset);
            section
        };

        let mut solver_indexer: SolverIndexer<'ast, T> = SolverIndexer::default();
        let mut unconstrained_variable_detector = UnconstrainedVariableDetector::new(&self);
        let mut count: usize = 0;

        // write constraints section
        let constraints = {
            let mut section = Section::new(SectionType::Constraints);
            section.set_offset(w.stream_position()?);

            let statements = self.statements.into_iter();
            for s in statements {
                if matches!(s, Statement::Constraint(..)) {
                    count += 1;
                }
                let s: Vec<Statement<T>> = solver_indexer
                    .fold_statement(s)
                    .into_iter()
                    .flat_map(|s| unconstrained_variable_detector.fold_statement(s))
                    .collect();
                for s in s {
                    serde_cbor::to_writer(&mut w, &s)?;
                }
            }

            section.set_length(w.stream_position()? - section.offset);
            section
        };

        // write solvers section
        let solvers = {
            let mut section = Section::new(SectionType::Solvers);
            section.set_offset(w.stream_position()?);

            serde_cbor::to_writer(&mut w, &solver_indexer.solvers)?;

            section.set_length(w.stream_position()? - section.offset);
            section
        };

        // write module map section
        let module_map = {
            let mut section = Section::new(SectionType::Solvers);
            section.set_offset(w.stream_position()?);

            serde_cbor::to_writer(&mut w, &self.module_map)?;

            section.set_length(w.stream_position()? - section.offset);
            section
        };

        let header = ProgHeader {
            curve_id: T::id(),
            constraint_count: count as u32,
            return_count: self.return_count as u32,
            sections: [parameters, constraints, solvers, module_map],
        };

        // rewind to write the header
        w.seek(SeekFrom::Start(header_start))?;
        header.write(&mut w)?;

        unconstrained_variable_detector
            .finalize()
            .map(|_| count)
            .map_err(|count| format!("Error: Found {} unconstrained variable(s)", count).into())
    }
}

pub struct UnwrappedStreamDeserializer<'de, R, T> {
    s: StreamDeserializer<'de, R, T>,
}

impl<'de, R: serde_cbor::de::Read<'de>, T: serde::Deserialize<'de>> Iterator
    for UnwrappedStreamDeserializer<'de, R, T>
{
    type Item = T;

    fn next(&mut self) -> Option<T> {
        self.s.next().and_then(|v| v.ok())
    }
}

impl<'de, T: Field, R: Read + Seek>
    ProgIterator<
        'de,
        T,
        UnwrappedStreamDeserializer<'de, serde_cbor::de::IoRead<R>, Statement<'de, T>>,
    >
{
    pub fn read(mut r: R, header: &ProgHeader) -> Self {
        assert_eq!(header.curve_id, T::id());

        let parameters = {
            let section = &header.sections[0];
            r.seek(std::io::SeekFrom::Start(section.offset)).unwrap();

            let mut p = serde_cbor::Deserializer::from_reader(r.by_ref());
            Vec::deserialize(&mut p)
                .map_err(|_| String::from("Cannot read parameters"))
                .unwrap()
        };

        let solvers = {
            let section = &header.sections[2];
            r.seek(std::io::SeekFrom::Start(section.offset)).unwrap();

            let mut p = serde_cbor::Deserializer::from_reader(r.by_ref());
            Vec::deserialize(&mut p)
                .map_err(|_| String::from("Cannot read solvers"))
                .unwrap()
        };

        let module_map = {
            let section = &header.sections[3];
            r.seek(std::io::SeekFrom::Start(section.offset)).unwrap();

            let mut p = serde_cbor::Deserializer::from_reader(r.by_ref());
            ModuleMap::deserialize(&mut p)
                .map_err(|_| String::from("Cannot read module map"))
                .unwrap()
        };

        let statements_deserializer = {
            let section = &header.sections[1];
            r.seek(std::io::SeekFrom::Start(section.offset)).unwrap();

            let p = serde_cbor::Deserializer::from_reader(r);
            let s = p.into_iter::<Statement<T>>();

            UnwrappedStreamDeserializer { s }
        };

        ProgIterator::new(
            parameters,
            statements_deserializer,
            header.return_count as usize,
            module_map,
            solvers,
        )
    }
}

#[cfg(all(feature = "bn128", feature = "bls12_381"))]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::Prog;
    use std::io::{Cursor, Seek, SeekFrom};
    use zokrates_field::{Bls12_381Field, Bn128Field};

    #[test]
    fn ser_deser_v2() {
        let p: Prog<Bn128Field> = Prog::default();

        let mut buffer = Cursor::new(vec![]);
        p.clone().serialize(&mut buffer).unwrap();

        // rewind back to the beginning of the file
        buffer.seek(SeekFrom::Start(0)).unwrap();

        // parse header
        let header = ProgHeader::read(&mut buffer).unwrap();

        // deserialize
        let deserialized_p = ProgIterator::read(buffer, &header);

        assert_eq!(p, deserialized_p.collect());

        let p: Prog<Bls12_381Field> = Prog::default();

        let mut buffer = Cursor::new(vec![]);
        p.clone().serialize(&mut buffer).unwrap();

        // rewind back to the beginning of the file
        buffer.seek(SeekFrom::Start(0)).unwrap();

        // parse header
        let header = ProgHeader::read(&mut buffer).unwrap();

        // deserialize
        let deserialized_p = ProgIterator::read(buffer, &header);

        assert_eq!(p, deserialized_p.collect());
    }
}
