use hyper::{Body, Method, Request, Response, StatusCode};
use md5;
use mime_guess::from_path;
use std::convert::Infallible;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::util::{encode_uri, format_date_time, get_depth, get_req_path};

pub async fn handle_request(req: Request<Body>) -> Result<Response<Body>, Infallible> {
    let resp;
    // webdav 访问路径前缀
    let server_prefix = "/webdav";
    let req_path = get_req_path(&req);
    // 要挂载的目录
    let base_dir = "/Users/halo/webdav/";
    let mut path = req_path.to_string();
    path.replace_range(0..server_prefix.len(), "");
    // 被访问资源绝对路径
    let full_path = Path::new(&base_dir).join(path.trim_start_matches('/'));
    if !full_path.exists() && !full_path.is_dir() || !req_path.starts_with("/webdav") {
        return Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::empty())
            .unwrap());
    }
    println!("req: {:?}", &req);

    // 实现各个 HTTP 方法
    let method = req.method();
    if method == Method::from_bytes(b"PROPFIND").unwrap() {
        println!("prop: {}, {:?}", path, full_path);
        let multistatus_xml = handle_propfind_resp(&req, full_path, server_prefix, base_dir);
        resp = Response::builder()
            .status(StatusCode::MULTI_STATUS)
            .header("Content-Type", "application/xml; charset=utf-8")
            .body(Body::from(multistatus_xml))
            .unwrap();
    } else if method == Method::from_bytes(b"COPY").unwrap() {
        resp = Response::new(Body::from("Hello, Webdav, COPY"));
    } else {
        match req.method() {
            &Method::OPTIONS => {
                let allow_methods = "OPTIONS, GET, HEAD, POST, PUT, DELETE, PROPFIND";
                resp = Response::builder()
                    .status(StatusCode::OK)
                    .header("Allow", allow_methods)
                    .header("DAV", "1")
                    .body(Body::empty())
                    .unwrap();
            }
            &Method::GET => {
                resp = handle_get_resp(&full_path).await;
            }
            &Method::PUT => {
                resp = Response::new(Body::from("Hello, Webdav, PUT"));
            }
            _ => {
                resp = Response::new(Body::from("Hello, Webdav"));
            }
        }
    }

    if method != Method::GET {
        println!("resp: {:?}", resp);
    }
    Ok(resp)
}

fn handle_propfind_resp(
    req: &Request<Body>,
    full_path: PathBuf,
    server_prefix: &str,
    base_dir: &str,
) -> String {
    let depth = get_depth(req);
    let mut multistatus_xml = String::new();
    multistatus_xml.push_str(r#"<?xml version="1.0" encoding="utf-8"?>"#);
    multistatus_xml.push_str(r#"<D:multistatus xmlns:D="DAV:">"#);
    if depth == "0" {
        generate_content_xml(
            &mut multistatus_xml,
            full_path,
            base_dir,
            server_prefix.clone(),
        );
    } else {
        for entry in fs::read_dir(&full_path).unwrap() {
            if let Ok(entry) = entry {
                let entry_path = entry.path();
                println!("sub: {:?}, {}", entry_path, base_dir);
                generate_content_xml(
                    &mut multistatus_xml,
                    entry_path,
                    base_dir,
                    server_prefix.clone(),
                );
            }
        }
    }

    multistatus_xml.push_str("</D:multistatus>\n");
    multistatus_xml
}

async fn handle_get_resp(full_path: &PathBuf) -> Response<Body> {
    let file = match File::open(full_path) {
        Ok(file) => file,
        Err(_) => {
            println!("not found");
            return Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::empty())
                .unwrap();
        }
    };
    let mime_type = from_path(full_path).first_or_octet_stream();
    // 读取文件内容
    let mut buffer = Vec::new();
    match file.take(usize::MAX as u64).read_to_end(&mut buffer) {
        Ok(_) => Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", mime_type.as_ref())
            .body(Body::from(buffer))
            .unwrap(),
        Err(_) => {
            println!("not found2");
            return Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::empty())
                .unwrap();
        }
    }
}

fn generate_content_xml(
    multistatus_xml: &mut String,
    entry_path: PathBuf,
    base_dir: &str,
    server_prefix: &str,
) {
    let mut server_prefix_with_suffix = server_prefix.to_string();
    if !server_prefix_with_suffix.ends_with("/") {
        server_prefix_with_suffix += "/";
    }
    let mut relative_path = entry_path.to_string_lossy().to_owned().to_string();
    relative_path.replace_range(0..base_dir.len(), &server_prefix_with_suffix);
    println!(
        "inner: {}---{}==={:?}",
        server_prefix_with_suffix, base_dir, entry_path
    );
    multistatus_xml.push_str("<D:response>\n");
    multistatus_xml.push_str(
        format!(
            "<D:href>{}</D:href>\n",
            format!("{}", encode_uri(&relative_path))
        )
        .as_str(),
    );
    multistatus_xml.push_str("<D:propstat>\n");
    multistatus_xml.push_str("<D:prop>\n");

    let is_dir = entry_path.is_dir();
    let metadata = fs::metadata(&entry_path).unwrap();
    let last_modified = metadata.modified().unwrap();
    if is_dir {
        multistatus_xml.push_str("<D:resourcetype><D:collection/></D:resourcetype>\n");
    } else {
        let mime_type = from_path(&entry_path).first_or_octet_stream().to_string();
        let content_length = metadata.len();
        multistatus_xml.push_str(format!("<D:resourcetype/>\n").as_str());
        multistatus_xml.push_str(
            format!(
                "<D:getcontentlength>{}</D:getcontentlength>\n",
                content_length
            )
            .as_str(),
        );
        multistatus_xml
            .push_str(format!("<D:getcontenttype>{}</D:getcontenttype>\n", mime_type).as_str());
    }
    let mtime = last_modified.duration_since(std::time::UNIX_EPOCH).unwrap();
    let mtime_secs = mtime.as_secs();
    let etag = md5::compute(mtime_secs.to_string());
    multistatus_xml.push_str(format!("<D:getetag>{:?}</D:getetag>\n", etag).as_str());
    multistatus_xml.push_str(
        format!(
            "<D:getlastmodified>{}</D:getlastmodified>\n",
            format_date_time(last_modified)
        )
        .as_str(),
    );
    multistatus_xml.push_str("<D:displayname/>\n");
    multistatus_xml.push_str("</D:prop>\n");
    multistatus_xml.push_str("<D:status>HTTP/1.1 200 OK</D:status>\n");
    multistatus_xml.push_str("</D:propstat>\n");
    multistatus_xml.push_str("</D:response>\n");
}

pub enum DavMethod {
    // 将资源从一个URI复制到另一个URI
    COPY,
    // 锁定一个资源。WebDAV支持共享锁和互斥锁。
    LOCK,
    // 创建集合（即目录）
    MKCOL,
    // 将资源从一个URI移动到另一个URI
    MOVE,
    // 从Web资源中检索以XML格式存储的属性。它也被重载，以允许一个检索远程系统的集合结构（也叫目录层次结构）
    PROPFIND,
    // 在单个原子性动作中更改和删除资源的多个属性
    PROPPATCH,
    // 解除资源的锁定
    UNLOCK,
}
