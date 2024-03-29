use geiger::{find_unsafe_in_file, Count, CounterBlock, IncludeTests, ScanFileError};
use git2::Repository;
use http::{header, Request, Response, StatusCode};
use serde::Serialize;
use std::{collections::HashMap, path::Path};
use tempdir::TempDir;
use url::Url;
use walkdir::{DirEntry, Error as WalkDirError, WalkDir};

#[derive(Serialize)]
// Thanks Globi for this actual black magic
#[serde(remote = "Count")]
struct NewCount {
    safe: u64,

    // Thanks let dumbqt = proxi; for this! <3
    #[serde(rename = "unsafe")]
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
            // No we can't remove those clone()s because we don't own type
            // so we can't derive copy ourselves on it
            //
            // And yes there's no performance difference between clone()ing
            // and copying a struct with primitives
            // Thanks &star_wars, Fenrir, ~~EYESqu~~ ~~eyes-chan~~ I mean seequ
            // C: <3
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

#[derive(Debug)]
pub enum ScanningError {
    WalkDirError(WalkDirError),
    ScanFileError(ScanFileError),
}
// Thanks Globi I totally didn't understand http's errors apparently lol
impl From<ScanningError> for http::Response<String> {
    fn from(error: ScanningError) -> Self {
        match error {
            ScanningError::WalkDirError(_) => Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body("Failed to traverse repo".to_owned())
                // We literally just created an error-y response so it's okay to unwrap_err
                .unwrap(),
            ScanningError::ScanFileError(error) => Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(format!("Failed to scan file {}", error))
                // Same as error above
                .unwrap(),
        }
    }
}

fn is_hidden(entry: &DirEntry) -> bool {
    entry
        .file_name()
        .to_str()
        .map(|s| s.starts_with("."))
        .unwrap_or(false)
}

// Reworked find_unsafe thanks to let dumbqt = proxi;
// Much less allocations now and probably more readable lol
pub fn find_unsafe<P: AsRef<Path>>(root: P) -> Result<CounterBlock, ScanningError> {
    let mut counter_block = CounterBlock::default();

    for entry in WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| !is_hidden(e))
    {
        let file = entry
            .map(|dir_entry| dir_entry.path().to_owned())
            .map_err(|err| ScanningError::WalkDirError(err))?;

        if file.to_str().map(|s| s.ends_with(".rs")).unwrap_or(false) {
            let file_metrics = find_unsafe_in_file(&file, IncludeTests::No)
                .map_err(|err| ScanningError::ScanFileError(err))?;

            counter_block = counter_block + file_metrics.counters;
        }
    }

    Ok(counter_block)
}

fn handler(request: Request<()>) -> http::Result<Response<String>> {
    let url = Url::parse(&request.uri().to_string()).unwrap();
    let hash_query: HashMap<_, _> = url.query_pairs().to_owned().collect();

    match (hash_query.get("user"), hash_query.get("repo")) {
        (Some(user), Some(repo)) => {
            let repo_url = format!("https://github.com/{}/{}", user, repo);
            // Thanks let dumbqt = proxi; for telling me about this crate!
            let temp_dir = TempDir::new(&repo).expect("Should be able to create a temp dir");

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

            let data = match find_unsafe(&temp_dir) {
                Ok(data) => data,
                Err(error) => return Ok(error.into()),
            };

            let formattable_data = Output::from(data);

            let response = Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/json")
                // Our serde_json implementation should never fail, okay to unwrap
                .body(serde_json::to_string_pretty(&formattable_data).unwrap())
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
