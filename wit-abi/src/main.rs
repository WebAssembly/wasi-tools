use anyhow::{bail, Context, Result};
use heck::*;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;
use structopt::StructOpt;
use wit_parser::abi::AbiVariant;
use wit_parser::*;

#[derive(Debug, StructOpt)]
struct Options {
    /// Indicates that no files are written and instead files are checked if
    /// they're up-to-date with the source files.
    #[structopt(long)]
    check: bool,

    /// Indicates whether ABI files are for guest exports (or if not specified
    /// if they're for guest imports).
    #[structopt(long)]
    export: bool,

    /// Files and/or directories to walk and look for `*.wit.md` files within.
    files: Vec<String>,
}

fn main() -> Result<()> {
    let options = Options::from_args();
    for arg in env::args().skip(1) {
        let path = Path::new(&arg);
        if path.is_dir() {
            options.render_dir(path)?;
        } else {
            options.render_file(path)?;
        }
    }
    Ok(())
}

impl Options {
    fn render_dir(&self, path: &Path) -> Result<()> {
        let cx = || format!("failed to read directory {:?}", path);
        for dir in path.read_dir().with_context(&cx)? {
            let dir = dir.with_context(&cx)?;
            let ty = dir.file_type().with_context(&cx)?;
            let path = dir.path();
            if ty.is_dir() {
                self.render_dir(&path)?;
            } else {
                self.render_file(&path)?;
            }
        }
        Ok(())
    }

    fn render_file(&self, path: &Path) -> Result<()> {
        let dir = match path.parent() {
            Some(parent) => parent,
            None => return Ok(()),
        };
        let filestem = match path.file_name().and_then(|s| s.to_str()) {
            Some(name) => match name.strip_suffix(".wit.md") {
                Some(name) => name,
                None => return Ok(()),
            },
            None => return Ok(()),
        };
        let interface = Interface::parse_file(path)
            .with_context(|| format!("failed to parse input {:?}", path))?;

        let mut markdown = Markdown {
            opts: self,
            src: String::new(),
            sizes: Default::default(),
            hrefs: HashMap::default(),
            funcs: 0,
            types: 0,
        };
        markdown.process(&interface);

        let dst = dir.join(&format!("{}.abi.md", filestem));
        if self.check {
            let prev =
                fs::read_to_string(&dst).with_context(|| format!("failed to read {:?}", dst))?;
            if prev != markdown.src {
                bail!("not up to date: {}", dst.display());
            }
        } else {
            fs::write(&dst, &markdown.src).with_context(|| format!("failed to write {:?}", dst))?;
            println!("wrote {}", dst.display());
        }
        Ok(())
    }
}

pub struct Markdown<'a> {
    opts: &'a Options,
    src: String,
    sizes: SizeAlign,
    hrefs: HashMap<String, String>,
    funcs: usize,
    types: usize,
}

impl Markdown<'_> {
    fn process(&mut self, iface: &Interface) {
        let variant = if self.opts.export {
            AbiVariant::GuestExport
        } else {
            AbiVariant::GuestImport
        };
        self.sizes.fill(variant, iface);

        for (id, ty) in iface.types.iter() {
            let name = match &ty.name {
                Some(name) => name,
                None => continue,
            };
            match &ty.kind {
                TypeDefKind::Record(record) => self.type_record(iface, id, name, record, &ty.docs),
                TypeDefKind::Variant(variant) => {
                    self.type_variant(iface, id, name, variant, &ty.docs)
                }
                TypeDefKind::Type(t) => self.type_alias(iface, id, name, t, &ty.docs),
                TypeDefKind::List(_)
                | TypeDefKind::Pointer(_)
                | TypeDefKind::ConstPointer(_)
                | TypeDefKind::PushBuffer(_)
                | TypeDefKind::PullBuffer(_) => {
                    self.type_alias(iface, id, name, &Type::Id(id), &ty.docs)
                }
            }
        }

        for func in iface.functions.iter() {
            if self.funcs == 0 {
                self.src.push_str("# Functions\n\n");
            }
            self.funcs += 1;

            self.src.push_str("----\n\n");
            self.src.push_str(&format!(
                "#### <a href=\"#{0}\" name=\"{0}\"></a> `",
                func.name.to_snake_case()
            ));
            self.hrefs
                .insert(func.name.clone(), format!("#{}", func.name.to_snake_case()));
            self.src.push_str(&func.name);
            self.src.push_str("` ");
            self.src.push_str("\n\n");
            self.docs(&func.docs);

            if func.params.len() > 0 {
                self.src.push_str("##### Params\n\n");
                for (name, ty) in func.params.iter() {
                    self.src.push_str(&format!(
                        "- <a href=\"#{f}.{p}\" name=\"{f}.{p}\"></a> `{}`: ",
                        name,
                        f = func.name.to_snake_case(),
                        p = name.to_snake_case(),
                    ));
                    self.print_ty(iface, ty, false);
                    self.src.push_str("\n");
                }
            }
            if func.results.len() > 0 {
                self.src.push_str("##### Results\n\n");
                for (name, ty) in func.results.iter() {
                    self.src.push_str(&format!(
                        "- <a href=\"#{f}.{p}\" name=\"{f}.{p}\"></a> `{}`: ",
                        name,
                        f = func.name.to_snake_case(),
                        p = name.to_snake_case(),
                    ));
                    self.print_ty(iface, ty, false);
                    self.src.push_str("\n");
                }
            }

            self.src.push_str("\n");
        }
    }

    fn type_record(
        &mut self,
        iface: &Interface,
        id: TypeId,
        name: &str,
        record: &Record,
        docs: &Docs,
    ) {
        self.print_type_header(name);
        self.src.push_str("record\n\n");
        self.print_type_info(id, docs);
        self.src.push_str("\n### Record Fields\n\n");
        for (i, field) in record.fields.iter().enumerate() {
            self.src.push_str(&format!(
                "- <a href=\"{r}.{f}\" name=\"{r}.{f}\"></a> [`{name}`](#{r}.{f}): ",
                r = name.to_snake_case(),
                f = field.name.to_snake_case(),
                name = field.name,
            ));
            self.hrefs.insert(
                format!("{}::{}", name, field.name),
                format!("#{}.{}", name.to_snake_case(), field.name.to_snake_case()),
            );
            self.print_ty(iface, &field.ty, false);
            self.src.push_str("\n\n");
            self.docs(&field.docs);
            if record.is_flags() {
                self.src.push_str(&format!("Bit: {}\n", i));
            }
            self.src.push_str("\n");
        }
    }

    fn type_variant(
        &mut self,
        iface: &Interface,
        id: TypeId,
        name: &str,
        variant: &Variant,
        docs: &Docs,
    ) {
        self.print_type_header(name);
        self.src.push_str("variant\n\n");
        self.print_type_info(id, docs);
        self.src.push_str("\n### Variant Cases\n\n");
        for case in variant.cases.iter() {
            self.src.push_str(&format!(
                "- <a href=\"{v}.{c}\" name=\"{v}.{c}\"></a> [`{name}`](#{v}.{c})",
                v = name.to_snake_case(),
                c = case.name.to_snake_case(),
                name = case.name,
            ));
            self.hrefs.insert(
                format!("{}::{}", name, case.name),
                format!("#{}.{}", name.to_snake_case(), case.name.to_snake_case()),
            );
            if let Some(ty) = &case.ty {
                self.src.push_str(": ");
                self.print_ty(iface, ty, false);
            }
            self.src.push_str("\n\n");
            self.docs(&case.docs);
            self.src.push_str("\n");
        }
    }

    fn type_alias(&mut self, iface: &Interface, id: TypeId, name: &str, ty: &Type, docs: &Docs) {
        self.print_type_header(name);
        self.print_ty(iface, ty, true);
        self.src.push_str("\n\n");
        self.print_type_info(id, docs);
        self.src.push_str("\n");
    }

    fn print_ty(&mut self, iface: &Interface, ty: &Type, skip_name: bool) {
        match ty {
            Type::U8 => self.src.push_str("`u8`"),
            Type::S8 => self.src.push_str("`s8`"),
            Type::U16 => self.src.push_str("`u16`"),
            Type::S16 => self.src.push_str("`s16`"),
            Type::U32 => self.src.push_str("`u32`"),
            Type::S32 => self.src.push_str("`s32`"),
            Type::U64 => self.src.push_str("`u64`"),
            Type::S64 => self.src.push_str("`s64`"),
            Type::F32 => self.src.push_str("`f32`"),
            Type::F64 => self.src.push_str("`f64`"),
            Type::Char => self.src.push_str("`char`"),
            Type::CChar => self.src.push_str("`c_char`"),
            Type::Usize => self.src.push_str("`usize`"),
            Type::Handle(id) => {
                self.src.push_str("handle<");
                self.src.push_str(&iface.resources[*id].name);
                self.src.push_str(">");
            }
            Type::Id(id) => {
                let ty = &iface.types[*id];
                if !skip_name {
                    if let Some(name) = &ty.name {
                        self.src.push_str("[`");
                        self.src.push_str(name);
                        self.src.push_str("`](#");
                        self.src.push_str(&name.to_snake_case());
                        self.src.push_str(")");
                        return;
                    }
                }
                match &ty.kind {
                    TypeDefKind::Type(t) => self.print_ty(iface, t, false),
                    TypeDefKind::Record(r) => {
                        assert!(r.is_tuple());
                        self.src.push_str("(");
                        for (i, f) in r.fields.iter().enumerate() {
                            if i > 0 {
                                self.src.push_str(", ");
                            }
                            self.print_ty(iface, &f.ty, false);
                        }
                        self.src.push_str(")");
                    }
                    TypeDefKind::Variant(v) => {
                        if v.is_bool() {
                            self.src.push_str("`bool`");
                        } else if let Some(t) = v.as_option() {
                            self.src.push_str("option<");
                            self.print_ty(iface, t, false);
                            self.src.push_str(">");
                        } else if let Some((ok, err)) = v.as_expected() {
                            self.src.push_str("expected<");
                            match ok {
                                Some(t) => self.print_ty(iface, t, false),
                                None => self.src.push_str("_"),
                            }
                            self.src.push_str(", ");
                            match err {
                                Some(t) => self.print_ty(iface, t, false),
                                None => self.src.push_str("_"),
                            }
                            self.src.push_str(">");
                        } else {
                            unreachable!()
                        }
                    }
                    TypeDefKind::List(Type::Char) => self.src.push_str("`string`"),
                    TypeDefKind::List(t) => {
                        self.src.push_str("list<");
                        self.print_ty(iface, t, false);
                        self.src.push_str(">");
                    }
                    TypeDefKind::PushBuffer(t) => {
                        self.src.push_str("push-buffer<");
                        self.print_ty(iface, t, false);
                        self.src.push_str(">");
                    }
                    TypeDefKind::PullBuffer(t) => {
                        self.src.push_str("pull-buffer<");
                        self.print_ty(iface, t, false);
                        self.src.push_str(">");
                    }
                    TypeDefKind::Pointer(t) => {
                        self.src.push_str("pointer<");
                        self.print_ty(iface, t, false);
                        self.src.push_str(">");
                    }
                    TypeDefKind::ConstPointer(t) => {
                        self.src.push_str("const-pointer<");
                        self.print_ty(iface, t, false);
                        self.src.push_str(">");
                    }
                }
            }
        }
    }

    fn docs(&mut self, docs: &Docs) {
        let docs = match &docs.contents {
            Some(docs) => docs,
            None => return,
        };
        for line in docs.lines() {
            self.src.push_str("  ");
            self.src.push_str(line.trim());
            self.src.push_str("\n");
        }
    }

    fn print_type_header(&mut self, name: &str) {
        if self.types == 0 {
            self.src.push_str("# Types\n\n");
        }
        self.types += 1;
        self.src.push_str(&format!(
            "## <a href=\"#{}\" name=\"{0}\"></a> `{}`: ",
            name.to_snake_case(),
            name,
        ));
        self.hrefs
            .insert(name.to_string(), format!("#{}", name.to_snake_case()));
    }

    fn print_type_info(&mut self, ty: TypeId, docs: &Docs) {
        self.docs(docs);
        self.src.push_str("\n");
        self.src
            .push_str(&format!("Size: {}, ", self.sizes.size(&Type::Id(ty))));
        self.src
            .push_str(&format!("Alignment: {}\n", self.sizes.align(&Type::Id(ty))));
    }
}
