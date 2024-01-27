//! Define the Delta actions that exist, and how to parse them out of [EngineData]

use std::{collections::HashMap, sync::Arc};

use tracing::debug;

use crate::{
    engine_data::{DataItem, DataVisitor, EngineData},
    schema::StructType,
    DeltaResult, EngineClient, Error,
};

/// Generic struct to allow us to visit a type or hold an error that the type couldn't be parsed
struct Vistitor<T> {
    extracted: Option<DeltaResult<T>>,
    extract_fn: fn(vals: &[Option<DataItem<'_>>]) -> DeltaResult<T>,
}

impl<T> Vistitor<T> {
    fn new(extract_fn: fn(vals: &[Option<DataItem<'_>>]) -> DeltaResult<T>) -> Self {
        Vistitor {
            extracted: None,
            extract_fn,
        }
    }
}

impl<T> DataVisitor for Vistitor<T> {
    fn visit(&mut self, vals: &[Option<DataItem<'_>>]) {
        self.extracted = Some((self.extract_fn)(vals));
    }
}

/// Generic struct to allow us to visit a type repeatedly or hold an error that the type couldn't be parsed
pub(crate) struct MultiVistitor<T> {
    pub(crate) extracted: Vec<DeltaResult<T>>,
    extract_fn: fn(vals: &[Option<DataItem<'_>>]) -> DeltaResult<T>,
}

impl<T> MultiVistitor<T> {
    pub(crate) fn new(extract_fn: fn(vals: &[Option<DataItem<'_>>]) -> DeltaResult<T>) -> Self {
        MultiVistitor {
            extracted: vec!(),
            extract_fn,
        }
    }
}

impl<T> DataVisitor for MultiVistitor<T> {
    fn visit(&mut self, vals: &[Option<DataItem<'_>>]) {
        self.extracted.push((self.extract_fn)(vals));
    }
}

macro_rules! extract_required_item {
    ($item: expr, $as_func: ident, $typ: expr, $err_msg_missing: expr, $err_msg_type: expr) => {
        $item.as_ref()
            .ok_or(Error::Extract($typ, $err_msg_missing))?
            .$as_func()
            .ok_or(Error::Extract($typ, $err_msg_type))?
    };
}

macro_rules! extract_opt_item {
    ($item: expr, $as_func: ident, $typ: expr, $err_msg_type: expr) => {
        $item.as_ref()
            .map(|item| {
                item.$as_func().ok_or(Error::Extract($typ, $err_msg_type))
            }).transpose()?
    };
}


#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Format {
    /// Name of the encoding for files in this table
    pub provider: String,
    /// A map containing configuration options for the format
    pub options: HashMap<String, String>,
}

impl Default for Format {
    fn default() -> Self {
        Self {
            provider: String::from("parquet"),
            options: HashMap::new(),
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Metadata {
    /// Unique identifier for this table
    pub id: String,
    /// User-provided identifier for this table
    pub name: Option<String>,
    /// User-provided description for this table
    pub description: Option<String>,
    /// Specification of the encoding for the files stored in the table
    pub format: Format,
    /// Schema of the table
    pub schema_string: String,
    /// Column names by which the data should be partitioned
    pub partition_columns: Vec<String>,
    /// The time when this metadata action is created, in milliseconds since the Unix epoch
    pub created_time: Option<i64>,
    /// Configuration options for the metadata action
    pub configuration: HashMap<String, Option<String>>,
}

impl Metadata {
    pub fn try_new_from_data(
        engine_client: &dyn EngineClient,
        data: &dyn EngineData,
    ) -> DeltaResult<Metadata> {
        let extractor = engine_client.get_data_extactor();
        let mut visitor = Vistitor::new(visit_metadata);
        let schema = StructType::new(vec![crate::actions::schemas::METADATA_FIELD.clone()]);
        extractor.extract(data, Arc::new(schema), &mut visitor);
        visitor.extracted.unwrap_or_else(|| Err(Error::Generic("Didn't get expected metadata".to_string())))
    }

    pub fn schema(&self) -> DeltaResult<StructType> {
        Ok(serde_json::from_str(&self.schema_string)?)
    }
}

fn visit_metadata(vals: &[Option<DataItem<'_>>]) -> DeltaResult<Metadata> {
    let id = vals[0]
        .as_ref()
        .expect("MetaData must have an id")
        .as_str()
        .expect("id must be str");
    let name = vals[1]
        .as_ref()
        .map(|name_data| name_data.as_str().expect("name must be a str").to_string());
    let description = vals[2].as_ref().map(|desc_data| {
        desc_data
            .as_str()
            .expect("description must be a str")
            .to_string()
    });
    // get format out of primitives
    let format_provider = vals[3]
        .as_ref()
        .expect("format.provider must exist")
        .as_str()
        .expect("format.provider must be a str")
        .to_string();

    // todo: extract relevant values out of the options map at vals[4]

    let schema_string = vals[5]
        .as_ref()
        .expect("schema_string must exist")
        .as_str()
        .expect("schema_string must be a str")
        .to_string();

    // todo: partition_columns from vals[6]

    let created_time = vals[7]
        .as_ref()
        .expect("Action must have a created_time")
        .as_i64()
        .expect("created_time must be i64");

    // todo: config vals from vals[8]

    Ok(Metadata {
        id: id.to_string(),
        name,
        description,
        format: Format {
            provider: format_provider,
            options: HashMap::new(),
        },
        schema_string,
        partition_columns: vec![],
        created_time: Some(created_time),
        configuration: HashMap::new(),
    })
}

#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct Protocol {
    /// The minimum version of the Delta read protocol that a client must implement
    /// in order to correctly read this table
    pub min_reader_version: i32,
    /// The minimum version of the Delta write protocol that a client must implement
    /// in order to correctly write this table
    pub min_writer_version: i32,
    /// A collection of features that a client must implement in order to correctly
    /// read this table (exist only when minReaderVersion is set to 3)
    pub reader_features: Option<Vec<String>>,
    /// A collection of features that a client must implement in order to correctly
    /// write this table (exist only when minWriterVersion is set to 7)
    pub writer_features: Option<Vec<String>>,
}

impl Protocol {
    pub fn try_new_from_data(
        engine_client: &dyn EngineClient,
        data: &dyn EngineData,
    ) -> DeltaResult<Protocol> {
        let extractor = engine_client.get_data_extactor();
        let mut visitor = ProtocolVisitor::default();
        let schema = StructType::new(vec![crate::actions::schemas::PROTOCOL_FIELD.clone()]);
        extractor.extract(data, Arc::new(schema), &mut visitor);
        visitor
            .extracted
            .ok_or(Error::Generic("Failed to extract protocol".to_string()))
    }
}

#[derive(Default)]
pub(crate) struct ProtocolVisitor {
    pub(crate) extracted: Option<Protocol>,
}

impl DataVisitor for ProtocolVisitor {
    fn visit(&mut self, vals: &[Option<DataItem<'_>>]) {
        let min_reader_version = vals[0]
            .as_ref()
            .expect("Protocol must have a minReaderVersion")
            .as_i32()
            .expect("minReaderVersion must be i32");
        let min_writer_version = vals[1]
            .as_ref()
            .expect("Protocol must have a minWriterVersion")
            .as_i32()
            .expect("minWriterVersion must be i32");

        let reader_features = vals[2].as_ref().map(|rf_di| {
            if let DataItem::StrList(lst) = rf_di {
                lst.iter().map(|f| f.to_string()).collect()
            } else {
                panic!("readerFeatures must be a string list")
            }
        });

        let writer_features = vals[3].as_ref().map(|rf_di| {
            if let DataItem::StrList(lst) = rf_di {
                lst.iter().map(|f| f.to_string()).collect()
            } else {
                panic!("readerFeatures must be a string list")
            }
        });

        let extracted = Protocol {
            min_reader_version,
            min_writer_version,
            reader_features,
            writer_features,
        };
        debug!("Extracted: {:#?}", extracted);
        self.extracted = Some(extracted)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Add {
    /// A relative path to a data file from the root of the table or an absolute path to a file
    /// that should be added to the table. The path is a URI as specified by
    /// [RFC 2396 URI Generic Syntax], which needs to be decoded to get the data file path.
    ///
    /// [RFC 2396 URI Generic Syntax]: https://www.ietf.org/rfc/rfc2396.txt
    pub path: String,

    /// A map from partition column to value for this logical file.
    pub partition_values: HashMap<String, Option<String>>,

    /// The size of this data file in bytes
    pub size: i64,

    /// The time this logical file was created, as milliseconds since the epoch.
    pub modification_time: i64,

    /// When `false` the logical file must already be present in the table or the records
    /// in the added file must be contained in one or more remove actions in the same version.
    pub data_change: bool,

    /// Contains [statistics] (e.g., count, min/max values for columns) about the data in this logical file.
    ///
    /// [statistics]: https://github.com/delta-io/delta/blob/master/PROTOCOL.md#Per-file-Statistics
    pub stats: Option<String>,

    /// Map containing metadata about this logical file.
    pub tags: HashMap<String, Option<String>>,

    /// Information about deletion vector (DV) associated with this add action
    //pub deletion_vector: Option<DeletionVectorDescriptor>,

    /// Default generated Row ID of the first row in the file. The default generated Row IDs
    /// of the other rows in the file can be reconstructed by adding the physical index of the
    /// row within the file to the base Row ID
    pub base_row_id: Option<i64>,

    /// First commit version in which an add action with the same path was committed to the table.
    pub default_row_commit_version: Option<i64>,
}

impl Add {
    pub fn try_new_from_data(
        engine_client: &dyn EngineClient,
        data: &dyn EngineData,
    ) -> DeltaResult<Add> {
        let extractor = engine_client.get_data_extactor();
        let mut visitor = Vistitor::new(visit_add);
        let schema = StructType::new(vec![crate::actions::schemas::ADD_FIELD.clone()]);
        extractor.extract(data, Arc::new(schema), &mut visitor);
        visitor.extracted.expect("Didn't get Add")
    }
}

pub(crate) fn visit_add(vals: &[Option<DataItem<'_>>]) -> DeltaResult<Add> {
    let path = extract_required_item!(
        vals[0],
        as_str,
        "Add",
        "Add must have path",
        "path must be str"
    );

    // TODO: Support partition_values

    let size = extract_required_item!(
        vals[2],
        as_i64,
        "Add",
        "Add must have size",
        "size must be i64"
    );

    let modification_time = extract_required_item!(
        vals[3],
        as_i64,
        "Add",
        "Add must have modification_time",
        "modification_time must be i64"
    );

    let data_change = extract_required_item!(
        vals[4],
        as_bool,
        "Add",
        "Add must have data_change",
        "modification_time must be bool"
    );

    let stats = extract_opt_item!(
        vals[5],
        as_str,
        "Add",
        "stats must be str"
    );

    // todo extract tags

    let base_row_id = extract_opt_item!(
        vals[7],
        as_i64,
        "Add",
        "base_row_id must be i64"
    );

    let default_row_commit_version = extract_opt_item!(
        vals[8],
        as_i64,
        "Add",
        "default_row_commit_version must be i64"
    );

    Ok(Add {
        path: path.to_string(),
        partition_values: HashMap::new(),
        size,
        modification_time,
        data_change,
        stats: stats.map(|s| s.to_string()),
        tags: HashMap::new(),
        base_row_id,
        default_row_commit_version,
    })
}
