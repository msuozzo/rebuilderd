use crate::config;
use crate::diffoscope::diffoscope;
use crate::download::download;
use crate::heartbeat::HeartBeat;
use crate::log;
use crate::proc;
use in_toto::crypto::PrivateKey;
use in_toto::runlib::in_toto_run;
use rebuilderd_common::api::v1::{ArtifactStatus, QueuedJobArtifact, RebuildArtifactReport};
use rebuilderd_common::errors::Context as _;
use rebuilderd_common::errors::*;
use rebuilderd_common::utils::zstd_compress;
use std::collections::HashMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tokio::time;

pub struct Context<'a> {
    pub artifacts: Vec<QueuedJobArtifact>,
    pub input_url: Option<String>,
    pub backend: config::Backend,
    pub build: config::Build,
    pub diffoscope: config::Diffoscope,
    pub privkey: &'a PrivateKey,
}

fn path_to_string(path: &Path) -> Result<String> {
    let s = path
        .to_str()
        .with_context(|| anyhow!("Path contains invalid characters: {:?}", path))?;
    Ok(s.to_string())
}

pub async fn compare_files(a: &Path, b: &Path) -> Result<bool> {
    let mut buf1 = [0u8; 4096];
    let mut buf2 = [0u8; 4096];

    info!("Comparing {:?} with {:?}", a, b);
    let mut f1 = File::open(a)
        .await
        .with_context(|| anyhow!("Failed to open {:?}", a))?;
    let mut f2 = File::open(b)
        .await
        .with_context(|| anyhow!("Failed to open {:?}", b))?;

    let mut pos = 0;
    loop {
        // read up to 4k bytes from the first file
        let n = f1.read_buf(&mut &mut buf1[..]).await?;

        // check if the first file is end-of-file
        if n == 0 {
            debug!("First file is at end-of-file");

            // check if other file is eof too
            let n = f2.read_buf(&mut &mut buf2[..]).await?;
            if n > 0 {
                info!("Files are not identical, {:?} is longer", b);
                return Ok(false);
            } else {
                return Ok(true);
            }
        }

        // check the same chunk in the other file
        match f2.read_exact(&mut buf2[..n]).await {
            Ok(n) => n,
            Err(err) if err.kind() == ErrorKind::UnexpectedEof => {
                info!("Files are not identical, {:?} is shorter", b);
                return Ok(false);
            }
            err => err?,
        };

        if buf1[..n] != buf2[..n] {
            // get the exact position
            // this can't panic because we've already checked the slices are not equal
            let pos = pos
                + buf1[..n]
                    .iter()
                    .zip(buf2[..n].iter())
                    .position(|(a, b)| a != b)
                    .unwrap();
            info!("Files {:?} and {:?} differ at position {}", a, b, pos);

            return Ok(false);
        }

        // advance the number of bytes that are equal
        pos += n;
    }
}

pub async fn rebuild_with_heartbeat(
    ctx: &Context<'_>,
    log: &mut log::Buffer,
    hb: &dyn HeartBeat,
) -> Result<Vec<RebuildArtifactReport>> {
    let mut rebuild = Box::pin(rebuild(ctx, log));
    loop {
        tokio::select! {
            res = &mut rebuild => {
                return res;
            },
            _ = time::sleep(hb.interval()) => hb.ping().await?,
        }
    }
}

pub async fn rebuild(
    ctx: &Context<'_>,
    log: &mut log::Buffer,
) -> Result<Vec<RebuildArtifactReport>> {
    // setup
    let tmp = tempfile::Builder::new().prefix("rebuilderd").tempdir()?;

    let inputs_dir = tmp.path().join("inputs");
    fs::create_dir(&inputs_dir).context("Failed to create inputs/ temp dir")?;

    let out_dir = tmp.path().join("out");
    fs::create_dir(&out_dir).context("Failed to create out/ temp dir")?;

    // download
    let mut artifacts = Vec::new();
    for artifact in &ctx.artifacts {
        let artifact_filename = download(&artifact.url, &inputs_dir)
            .await
            .with_context(|| {
                anyhow!(
                    "Failed to download original package from {:?}",
                    artifact.url
                )
            })?;
        let artifact_path = inputs_dir.join(&artifact_filename);
        artifacts.push((artifact.clone(), artifact_filename, artifact_path));
    }

    let input_filename = if let Some(input_url) = &ctx.input_url {
        download(input_url, &inputs_dir)
            .await
            .with_context(|| anyhow!("Failed to download build input from {:?}", input_url))?
    } else {
        artifacts
            .first()
            .context("Failed to use first artifact as build input")?
            .1
            .to_owned()
    };
    let input_path = inputs_dir.join(&input_filename);

    // rebuild
    verify(ctx, log, &out_dir, &input_path).await?;

    // process results
    let mut results = Vec::new();
    for (artifact, artifact_filename, artifact_path) in artifacts {
        let output_path = out_dir.join(&artifact_filename);

        if !output_path.exists() {
            info!(
                "No output artifact found, marking as BAD: {:?}",
                output_path
            );

            results.push(RebuildArtifactReport {
                name: artifact.name,
                diffoscope: None,
                attestation: None,
                status: ArtifactStatus::Bad,
                stabilizer: None,
            });
            continue;
        }

        // phase 1: bit-for-bit comparison
        let identical = compare_files(&artifact_path, &output_path).await?;

        // phase 2: if the artifacts differ and the backend has a stabilizer,
        // pass both of them through it and compare the stabilized copies
        let stabilized = match &ctx.backend.stabilizer {
            Some(stabilizer) if !identical => Some(
                stabilize_artifacts(
                    stabilizer,
                    &artifact_path,
                    &output_path,
                    &tmp.path().join("stabilized"),
                    &ctx.build,
                    log,
                )
                .await?,
            ),
            _ => None,
        };
        let stabilized_identical = if let Some(stabilized) = &stabilized {
            compare_files(&stabilized.upstream, &stabilized.rebuild).await?
        } else {
            false
        };

        let result = if identical || stabilized_identical {
            // stabilized copies only exist if the artifacts are not bit-for-bit identical
            if stabilized.is_some() {
                info!(
                    "Stabilized artifacts are identical, marking as GOOD: {:?}",
                    output_path
                );
            } else {
                info!(
                    "Output artifacts is identical, marking as GOOD: {:?}",
                    output_path
                );
            }

            let mut res = RebuildArtifactReport {
                name: artifact.name,
                diffoscope: None,
                attestation: None,
                status: ArtifactStatus::Good,
                stabilizer: stabilized.as_ref().map(|s| s.name.clone()),
            };

            info!("Generating signed link");
            let (materials, products, lstrip) = if let Some(stabilized) = &stabilized {
                // The rebuilt artifact only matches upstream after stabilizing,
                // so the product of this link is the stabilized rebuilt artifact.
                // The materials are the build input, the upstream artifact, the
                // unmodified rebuilt artifact and the stabilizer itself, so the
                // link also pins the transformation that has been applied.
                let mut materials = vec![path_to_string(&input_path)?];
                if artifact_path != input_path {
                    materials.push(path_to_string(&artifact_path)?);
                }
                materials.push(path_to_string(&output_path)?);
                materials.push(path_to_string(&stabilized.path)?);
                // the upstream and the rebuilt artifact share a file name,
                // only strip the tmp dir so inputs/, out/ and stabilized/
                // can still be told apart
                let lstrip = vec![format!("{}/", path_to_string(tmp.path())?)];
                (
                    materials,
                    vec![path_to_string(&stabilized.rebuild)?],
                    lstrip,
                )
            } else {
                let lstrip = vec![
                    format!("{}/", path_to_string(&inputs_dir)?),
                    format!("{}/", path_to_string(&out_dir)?),
                ];
                (
                    vec![path_to_string(&input_path)?],
                    vec![path_to_string(&output_path)?],
                    lstrip,
                )
            };
            let materials = materials.iter().map(String::as_str).collect::<Vec<_>>();
            let products = products.iter().map(String::as_str).collect::<Vec<_>>();
            let lstrip = lstrip.iter().map(String::as_str).collect::<Vec<_>>();

            match in_toto_run(
                &format!("rebuild {}", artifact_filename.to_str().unwrap()),
                None,
                &materials,
                &products,
                &[],
                Some(ctx.privkey),
                Some(&["sha512", "sha256"]),
                Some(&lstrip),
            ) {
                Ok(signed_link) => {
                    info!("Signed link generated");

                    let attestation = serde_json::to_string(&signed_link)
                        .context("Failed to serialize attestation")?;

                    let encoded_attestation = zstd_compress(attestation.as_bytes())
                        .await
                        .map_err(Error::from)?;

                    res.attestation = Some(encoded_attestation);
                }
                Err(err) => warn!("Failed to generate in-toto attestation: {:#?}", err),
            }

            res
        } else {
            info!("Output artifact differs, marking as BAD: {:?}", output_path);

            let mut res = RebuildArtifactReport {
                name: artifact.name,
                diffoscope: None,
                attestation: None,
                status: ArtifactStatus::Bad,
                stabilizer: None,
            };

            // generate diffoscope diff if enabled
            if ctx.diffoscope.enabled {
                // if both artifacts have been stabilized, diff the stabilized
                // copies instead: the diff then only shows what the stabilizer
                // could not account for
                let (a, b) = if let Some(stabilized) = &stabilized {
                    (&stabilized.upstream, &stabilized.rebuild)
                } else {
                    (&artifact_path, &output_path)
                };
                let diff = diffoscope(a, b, &ctx.diffoscope)
                    .await
                    .context("Failed to run diffoscope")?;

                let encoded_diffoscope =
                    zstd_compress(diff.as_bytes()).await.map_err(Error::from)?;

                res.diffoscope = Some(encoded_diffoscope);
            }

            res
        };

        results.push(result);
    }

    Ok(results)
}

/// Stabilized copies of an upstream and a rebuilt artifact
struct Stabilized {
    /// The file name of the stabilizer, as configured
    name: String,
    /// The absolute path of the stabilizer
    path: PathBuf,
    upstream: PathBuf,
    rebuild: PathBuf,
}

/// Passes both the upstream and the rebuilt artifact through the stabilizer
async fn stabilize_artifacts(
    stabilizer: &Path,
    artifact_path: &Path,
    output_path: &Path,
    stabilized_dir: &Path,
    build: &config::Build,
    log: &mut log::Buffer,
) -> Result<Stabilized> {
    let name = stabilizer
        .file_name()
        .with_context(|| anyhow!("Invalid stabilizer path: {:?}", stabilizer))?
        .to_string_lossy()
        .into_owned();
    let path = fs::canonicalize(stabilizer)
        .with_context(|| anyhow!("Failed to resolve stabilizer {:?}", stabilizer))?;

    info!(
        "Output artifact differs, stabilizing both artifacts with {:?}",
        path
    );
    let upstream = stabilize(
        &path,
        artifact_path,
        &stabilized_dir.join("upstream"),
        build,
        log,
    )
    .await?;
    let rebuild = stabilize(
        &path,
        output_path,
        &stabilized_dir.join("rebuild"),
        build,
        log,
    )
    .await?;

    Ok(Stabilized {
        name,
        path,
        upstream,
        rebuild,
    })
}

/// Runs `<stabilizer> <src> <dst_dir>/<file name of src>` and returns the path
/// of the stabilized copy.
async fn stabilize(
    stabilizer: &Path,
    src: &Path,
    dst_dir: &Path,
    build: &config::Build,
    log: &mut log::Buffer,
) -> Result<PathBuf> {
    fs::create_dir_all(dst_dir)
        .with_context(|| anyhow!("Failed to create stabilizer output dir {:?}", dst_dir))?;
    let dst = dst_dir.join(
        src.file_name()
            .with_context(|| anyhow!("Artifact path has no file name: {:?}", src))?,
    );

    let timeout = build.timeout.unwrap_or(3600 * 24); // 24h
    let opts = proc::Options {
        timeout: Duration::from_secs(timeout),
        kill_at_size_limit: false,
        passthrough: !build.silent,
        envs: HashMap::new(),
    };

    if !proc::run(stabilizer, &[src, dst.as_path()], opts, log).await? {
        bail!("Stabilizer {:?} failed on {:?}", stabilizer, src);
    }
    if !dst.exists() {
        bail!("Stabilizer {:?} did not write {:?}", stabilizer, dst);
    }

    Ok(dst)
}

async fn verify(
    ctx: &Context<'_>,
    log: &mut log::Buffer,
    out_dir: &Path,
    input_path: &Path,
) -> Result<()> {
    let bin = &ctx.backend.path;
    let timeout = ctx.build.timeout.unwrap_or(3600 * 24); // 24h

    let mut envs = HashMap::new();
    envs.insert("REBUILDERD_OUTDIR".into(), path_to_string(out_dir)?);

    let opts = proc::Options {
        timeout: Duration::from_secs(timeout),
        kill_at_size_limit: false,
        passthrough: !ctx.build.silent,
        envs,
    };

    proc::run(bin.as_ref(), &[input_path], opts, log).await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn compare_files_equal() {
        let equal = compare_files(Path::new("src/main.rs"), Path::new("src/main.rs"))
            .await
            .unwrap();
        assert!(equal);
    }

    #[tokio::test]
    async fn compare_files_not_equal1() {
        let equal = compare_files(Path::new("src/main.rs"), Path::new("Cargo.toml"))
            .await
            .unwrap();
        assert!(!equal);
    }

    #[tokio::test]
    async fn compare_files_not_equal2() {
        let equal = compare_files(Path::new("Cargo.toml"), Path::new("src/main.rs"))
            .await
            .unwrap();
        assert!(!equal);
    }

    #[tokio::test]
    async fn compare_large_files_equal() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a"), [0u8; 4096 * 100]).unwrap();
        fs::write(dir.path().join("b"), [0u8; 4096 * 100]).unwrap();
        let equal = compare_files(&dir.path().join("a"), &dir.path().join("b"))
            .await
            .unwrap();
        assert!(equal);
    }

    #[tokio::test]
    async fn compare_large_files_not_equal() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a"), [0u8; 4096 * 100]).unwrap();
        fs::write(dir.path().join("b"), [1u8; 4096 * 100]).unwrap();
        let equal = compare_files(&dir.path().join("a"), &dir.path().join("b"))
            .await
            .unwrap();
        assert!(!equal);
    }

    fn write_stabilizer(dir: &Path, script: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("stabilizer.sh");
        fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    async fn run_stabilizer(script: &str) -> (tempfile::TempDir, PathBuf, Result<PathBuf>) {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("foo.pkg");
        fs::write(&src, b"hello world\n").unwrap();
        let stabilizer = write_stabilizer(dir.path(), script);

        let mut log = log::Buffer::new(None, None);
        let res = stabilize(
            &stabilizer,
            &src,
            &dir.path().join("stabilized/upstream"),
            &config::Build::default(),
            &mut log,
        )
        .await;
        (dir, src, res)
    }

    #[tokio::test]
    async fn stabilize_identity() {
        let (dir, src, res) = run_stabilizer(r#"cp "$1" "$2""#).await;
        let dst = res.unwrap();
        assert_eq!(dst, dir.path().join("stabilized/upstream/foo.pkg"));
        assert!(compare_files(&src, &dst).await.unwrap());
    }

    #[tokio::test]
    async fn stabilize_failure() {
        let (_dir, _src, res) = run_stabilizer("exit 1").await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn stabilize_no_output() {
        // exits successfully but doesn't write the output file
        let (_dir, _src, res) = run_stabilizer("exit 0").await;
        assert!(res.is_err());
    }
}
