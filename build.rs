use std::{env, fs, path::Path};

fn main() {
    let root = Path::new("web/dist");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut files = Vec::new();
    collect(root, root, &mut files);
    files.sort();
    let out = Path::new(&env::var_os("OUT_DIR").expect("OUT_DIR")).join("web_assets.rs");
    let mut generated = String::from("pub static WEB_ASSETS: &[(&str, &[u8], &str)] = &[\n");
    for file in files {
        let relative = file
            .strip_prefix(root)
            .expect("asset under dist")
            .to_string_lossy()
            .replace('\\', "/");
        let path = file.canonicalize().expect("canonical asset path");
        println!("cargo:rerun-if-changed={}", path.display());
        let mime = match file
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("")
        {
            "html" => "text/html; charset=utf-8",
            "js" | "mjs" => "text/javascript; charset=utf-8",
            "css" => "text/css; charset=utf-8",
            "svg" => "image/svg+xml",
            "json" => "application/json",
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "ico" => "image/x-icon",
            "woff" => "font/woff",
            "woff2" => "font/woff2",
            "ttf" => "font/ttf",
            "wasm" => "application/wasm",
            "txt" => "text/plain; charset=utf-8",
            _ => "application/octet-stream",
        };
        generated.push_str(&format!(
            "    ({relative:?}, include_bytes!({:?}) as &[u8], {mime:?}),\n",
            path
        ));
    }
    generated.push_str("];\n");
    fs::write(out, generated).expect("write generated asset table");
}

fn collect(root: &Path, dir: &Path, files: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, files);
        } else if path.is_file() {
            files.push(path);
        }
    }
    let _ = root;
}
