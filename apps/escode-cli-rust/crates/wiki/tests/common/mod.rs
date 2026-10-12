#![allow(dead_code)]
//! 测试夹具：临时 home / 项目目录、Git 仓库布局、真实 OOXML 与 PDF 文件生成。
use escode_cli_wiki::WikiService;
use std::io::Write;
use std::path::{Path, PathBuf};

pub struct Env {
    pub home: tempfile::TempDir,
    pub work: tempfile::TempDir,
}

impl Env {
    pub fn new() -> Self {
        Self {
            home: tempfile::tempdir().unwrap(),
            work: tempfile::tempdir().unwrap(),
        }
    }
    /// 新建一个带 origin remote 的 Git 仓库目录（只写 .git/config，不调用 git）。
    pub fn repo(&self, name: &str, remote: Option<&str>) -> PathBuf {
        let dir = self.work.path().join(name);
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        let config = match remote {
            Some(url) => format!("[core]\n\tbare = false\n[remote \"origin\"]\n\turl = {url}\n"),
            None => "[core]\n\tbare = false\n".to_owned(),
        };
        std::fs::write(dir.join(".git/config"), config).unwrap();
        dir
    }
    pub fn plain(&self, name: &str) -> PathBuf {
        let dir = self.work.path().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
    pub fn service(&self, cwd: &Path) -> WikiService {
        WikiService::new(cwd.to_owned(), None, self.home.path().to_owned())
    }
    pub fn user_config(&self, value: serde_json::Value) {
        let dir = self.home.path().join(".escode/cli");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.json"), value.to_string()).unwrap();
    }
}

pub fn write(path: &Path, text: &str) -> PathBuf {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, text).unwrap();
    path.to_owned()
}

pub fn zip_bytes(parts: &[(&str, &str)]) -> Vec<u8> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut cursor);
        let options = zip::write::SimpleFileOptions::default();
        for (name, content) in parts {
            zip.start_file(*name, options).unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }
    cursor.into_inner()
}

const W: &str = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"";

pub fn para(style: Option<&str>, text: &str) -> String {
    let style = style
        .map(|s| format!("<w:pPr><w:pStyle w:val=\"{s}\"/></w:pPr>"))
        .unwrap_or_default();
    format!("<w:p>{style}<w:r><w:t xml:space=\"preserve\">{text}</w:t></w:r></w:p>")
}

pub fn docx(body: &str) -> Vec<u8> {
    let document =
        format!("<?xml version=\"1.0\"?><w:document {W}><w:body>{body}</w:body></w:document>");
    let styles = format!(
        "<w:styles {W}><w:style w:styleId=\"1\"><w:name w:val=\"heading 1\"/></w:style><w:style w:styleId=\"Heading2\"><w:name w:val=\"heading 2\"/></w:style><w:style w:styleId=\"Title\"><w:name w:val=\"Title\"/></w:style></w:styles>"
    );
    let rels = "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId9\" Type=\"hyperlink\" Target=\"https://example.com/errors\" TargetMode=\"External\"/></Relationships>";
    let footnotes = format!(
        "<w:footnotes {W}><w:footnote w:id=\"0\"><w:p><w:r><w:t>sep</w:t></w:r></w:p></w:footnote><w:footnote w:id=\"2\"><w:p><w:r><w:t>Footnote: see RFC 7807.</w:t></w:r></w:p></w:footnote></w:footnotes>"
    );
    let core = "<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\"><dc:title>Engineering Standards</dc:title><dc:creator>QA</dc:creator></cp:coreProperties>";
    zip_bytes(&[
        ("[Content_Types].xml", "<Types/>"),
        ("word/document.xml", &document),
        ("word/styles.xml", &styles),
        ("word/_rels/document.xml.rels", rels),
        ("word/footnotes.xml", &footnotes),
        ("docProps/core.xml", core),
    ])
}

/// sheets: (名称, 是否隐藏, sheetData 内 XML, 额外 XML 如 mergeCells)
pub fn xlsx(shared: &[&str], sheets: &[(&str, bool, &str, &str)]) -> Vec<u8> {
    let mut workbook = String::from(
        "<workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><sheets>",
    );
    let mut rels = String::from(
        "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
    );
    let mut parts: Vec<(String, String)> = vec![];
    for (i, (name, hidden, data, extra)) in sheets.iter().enumerate() {
        let state = if *hidden { " state=\"hidden\"" } else { "" };
        workbook.push_str(&format!(
            "<sheet name=\"{name}\" sheetId=\"{}\"{state} r:id=\"rId{}\"/>",
            i + 1,
            i + 1
        ));
        rels.push_str(&format!(
            "<Relationship Id=\"rId{}\" Target=\"worksheets/sheet{}.xml\"/>",
            i + 1,
            i + 1
        ));
        parts.push((format!("xl/worksheets/sheet{}.xml", i + 1), format!("<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData>{data}</sheetData>{extra}</worksheet>")));
    }
    workbook.push_str("</sheets><definedNames><definedName name=\"Rates\">Fields!$B$2:$B$3</definedName></definedNames></workbook>");
    rels.push_str("</Relationships>");
    let sst = format!(
        "<sst>{}</sst>",
        shared
            .iter()
            .map(|s| format!("<si><t>{s}</t></si>"))
            .collect::<String>()
    );
    let mut all: Vec<(&str, &str)> = vec![
        ("[Content_Types].xml", "<Types/>"),
        ("xl/workbook.xml", &workbook),
        ("xl/_rels/workbook.xml.rels", &rels),
        ("xl/sharedStrings.xml", &sst),
    ];
    for (n, c) in &parts {
        all.push((n, c));
    }
    zip_bytes(&all)
}

pub fn pptx(slides: &[(&str, &str, Option<&str>)]) -> Vec<u8> {
    let mut presentation = String::from("<p:presentation xmlns:p=\"p\" xmlns:r=\"r\"><p:sldIdLst>");
    let mut rels = String::from("<Relationships>");
    let mut parts: Vec<(String, String)> = vec![];
    for (i, (title, body, notes)) in slides.iter().enumerate() {
        let n = i + 1;
        presentation.push_str(&format!("<p:sldId id=\"{}\" r:id=\"rId{n}\"/>", 255 + n));
        rels.push_str(&format!(
            "<Relationship Id=\"rId{n}\" Target=\"slides/slide{n}.xml\"/>"
        ));
        parts.push((format!("ppt/slides/slide{n}.xml"), format!("<p:sld xmlns:p=\"p\" xmlns:a=\"a\"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:nvPr><p:ph type=\"title\"/></p:nvPr></p:nvSpPr><p:txBody><a:p><a:r><a:t>{title}</a:t></a:r></a:p></p:txBody></p:sp><p:sp><p:txBody><a:p><a:r><a:t>{body}</a:t></a:r></a:p></p:txBody></p:sp><p:pic/></p:spTree></p:cSld></p:sld>")));
        if let Some(text) = notes {
            parts.push((format!("ppt/slides/_rels/slide{n}.xml.rels"), format!("<Relationships><Relationship Id=\"rIdN\" Target=\"../notesSlides/notesSlide{n}.xml\"/></Relationships>")));
            parts.push((format!("ppt/notesSlides/notesSlide{n}.xml"), format!("<p:notes xmlns:p=\"p\" xmlns:a=\"a\"><p:sp><p:txBody><a:p><a:r><a:t>{text}</a:t></a:r></a:p></p:txBody></p:sp></p:notes>")));
        }
    }
    presentation.push_str("</p:sldIdLst></p:presentation>");
    rels.push_str("</Relationships>");
    let mut all: Vec<(&str, &str)> = vec![
        ("[Content_Types].xml", "<Types/>"),
        ("ppt/presentation.xml", &presentation),
        ("ppt/_rels/presentation.xml.rels", &rels),
    ];
    for (n, c) in &parts {
        all.push((n, c));
    }
    zip_bytes(&all)
}

/// 用 lopdf 生成真实的文本型 PDF：每页若干行文字（Helvetica）；空字符串页 = 无文本层（模拟扫描页）。
pub fn pdf(pages: &[&[&str]]) -> Vec<u8> {
    use lopdf::content::{Content, Operation};
    use lopdf::{Document, Object, Stream, dictionary};
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let font_id = doc.add_object(
        dictionary! {"Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica"},
    );
    let resources_id = doc.add_object(dictionary! {"Font" => dictionary! {"F1" => font_id}});
    let mut kids = vec![];
    for lines in pages {
        let mut ops = vec![];
        for (i, line) in lines.iter().enumerate() {
            ops.push(Operation::new("BT", vec![]));
            ops.push(Operation::new("Tf", vec!["F1".into(), 12.into()]));
            ops.push(Operation::new(
                "Td",
                vec![72.into(), (720 - (i as i64) * 20).into()],
            ));
            ops.push(Operation::new("Tj", vec![Object::string_literal(*line)]));
            ops.push(Operation::new("ET", vec![]));
        }
        let content = Content { operations: ops };
        let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
        let page_id = doc.add_object(dictionary! {"Type" => "Page", "Parent" => pages_id, "Contents" => content_id, "Resources" => resources_id, "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()]});
        kids.push(page_id.into());
    }
    let count = kids.len() as i64;
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {"Type" => "Pages", "Kids" => kids, "Count" => count}),
    );
    let catalog_id = doc.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages_id});
    doc.trailer.set("Root", catalog_id);
    let mut out = vec![];
    doc.save_to(&mut out).unwrap();
    out
}
