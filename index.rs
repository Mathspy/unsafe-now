use geiger::{find_unsafe_in_file, Count, CounterBlock, IncludeTests, ScanFileError};
use git2::Repository;
use http::{header, Request, Response, StatusCode};
use serde::Serialize;
use std::{collections::HashMap, fs, path::Path};
use url::Url;
use walkdir::{DirEntry, Error as WalkDirError, WalkDir};

#[derive(Serialize)]
#[serde(remote = "Count")]
struct NewCount {
    safe: u64,
    unsafe_: u64,
}

#[derive(Serialize)]
struct Output {
    #[serde(with = "NewCount")]
    functions: Count,

    #[serde(with = "NewCount")]
    exprs: Count,

    #[serde(with = "NewCount")]
    item_impls: Count,

    #[serde(with = "NewCount")]
    item_traits: Count,

    #[serde(with = "NewCount")]
    methods: Count,

    #[serde(with = "NewCount")]
    total: Count,
}

impl From<CounterBlock> for Output {
    fn from(counter_block: CounterBlock) -> Self {
        Output {
            // TODO: Ask Rust server if theere is a way to avoid these clone()s
            functions: counter_block.functions.clone(),
            exprs: counter_block.exprs.clone(),
            item_impls: counter_block.item_impls.clone(),
            item_traits: counter_block.item_traits.clone(),
            methods: counter_block.methods.clone(),
            total: counter_block.functions
                + counter_block.exprs
                + counter_block.item_impls
                + counter_block.item_traits
                + counter_block.methods,
        }
    }
}

// fn serialize_output_field<S>(serializer: S, count: &Count, name: &'static str) -> Result<S::Ok, S::Error>
//     where
//         S: Serializer,
// {
//     let mut s = serializer.serialize_struct(name, 2)?;
//     s.serialize_field("safe", &count.safe)?;
//     s.serialize_field("unsafe", &count.unsafe_)?;
//     s.end()
// }

// impl Serialize for Output {
//     fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
//     where
//         S: Serializer,
//     {
//         let functions = serialize_output_field(serializer, &self.functions, "functions")?;
//         let exprs = serialize_output_field(serializer, &self.exprs, "exprs")?;
//         let item_impls = serialize_output_field(serializer, &self.item_impls, "item_impls")?;
//         let item_traits = serialize_output_field(serializer, &self.item_traits, "item_traits")?;
//         let methods = serialize_output_field(serializer, &self.methods, "methods")?;

//         let mut s = serializer.serialize_struct("CounterBlock", 5)?;
//         s.serialize_field("functions", &functions)?;
//         s.serialize_field("exprs", &exprs)?;
//         s.serialize_field("item_impls", &item_impls)?;
//         s.serialize_field("item_traits", &item_traits)?;
//         s.serialize_field("methods", &methods)?;
//         s.end()
//     }
// }

#[derive(Debug)]
pub enum UnsafeError {
    WalkDirError(WalkDirError),
    ScanFileError(ScanFileError),
}
impl From<UnsafeError> for http::Error {
    fn from(error: UnsafeError) -> Self {
        match error {
            UnsafeError::WalkDirError(_) => Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body("Failed to traverse repo")
                // We literally just created an error-y response so it's okay to unwrap_err
                .unwrap_err(),
            UnsafeError::ScanFileError(error) => Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(format!("Failed to scan file {}", error))
                // Same as error above
                .unwrap_err(),
        }
    }
}

// Yes I added this because .map doesn't exist for every type
// No I am no obsessed with chaining
// ....
// Maybe a little
trait Map {
    type error;

    fn map<F>(self, f: F) -> Result<Self, Self::error>
    where
        Self: Sized,
        F: FnOnce(Self) -> Result<Self, Self::error>;
}
impl Map for CounterBlock {
    type error = UnsafeError;

    fn map<F>(self, f: F) -> Result<Self, Self::error>
    where
        Self: Sized,
        F: FnOnce(Self) -> Result<Self, Self::error>,
    {
        f(self)
    }
}

fn is_hidden(entry: &DirEntry) -> bool {
    entry
        .file_name()
        .to_str()
        .map(|s| s.starts_with("."))
        .unwrap_or(false)
}

pub fn find_unsafe<P: AsRef<Path>>(root: P) -> Result<CounterBlock, UnsafeError> {
    WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| !is_hidden(e))
        // The reason I don't use filter_map is because I don't want to swallow errors
        .map(|entry| {
            entry
                .map(|dir_entry| dir_entry.path().to_owned())
                .map_err(|err| UnsafeError::WalkDirError(err))
        })
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .filter(|entry| entry.to_str().map(|s| s.ends_with(".rs")).unwrap_or(false))
        .map(|file| {
            find_unsafe_in_file(file, IncludeTests::No)
                .map_err(|err| UnsafeError::ScanFileError(err))
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .fold(
            CounterBlock::default(),
            |mut counter_block, file_metrics| {
                counter_block = counter_block + file_metrics.counters;
                counter_block
            },
        )
        .map(|counter_block| Ok(counter_block))
}

fn handler(request: Request<()>) -> http::Result<Response<String>> {
    let url = Url::parse(&request.uri().to_string()).unwrap();
    let hash_query: HashMap<_, _> = url.query_pairs().to_owned().collect();

    match (hash_query.get("user"), hash_query.get("repo")) {
        (Some(user), Some(repo)) => {
            let identifer = format!("{}/{}", user, repo);
            let repo_url = format!("https://github.com/{}", identifer);
            let temp_dir = Path::new("/tmp/").join(identifer);

            match Repository::clone(&repo_url, &temp_dir) {
                Ok(_) => (),
                Err(e) => {
                    return Response::builder()
                        .status(StatusCode::BAD_REQUEST)
                        .body(format!(
                            "Failed to clone {}\n> {:?}: {}",
                            repo_url,
                            e.code(),
                            e.to_string(),
                        ));
                }
            };

            let something = Output::from(find_unsafe(&temp_dir)?);

            // This should never fail because we literally just created this directory
            // So it's okay to unwrap
            fs::remove_dir_all(&temp_dir).unwrap();

            let response = Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/json")
                // Our serde_json implementation should never fail, okay to unwrap
                .body(serde_json::to_string_pretty(&something).unwrap())
                .expect("Failed to render response");

            Ok(response)
        }

        _ => Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body("BAD REQUEST.\nUsage instruction: /<github_username>/<github_repo>/".to_string()),
    }
}

// For local testing:
// fn main() {
//     let test = handler(
//         Request::get("https://unsafe-now-awqpllhtf.now.sh/?user=amethyst&repo=rendy")
//             .body(())
//             .unwrap(),
//     );

//     dbg!(test.unwrap().body());

//     // dbg!(find_unsafe("./amethyst/rendy"));
// }