use std::fs;
use std::path::Path;
use swc_common::{FileName, SourceMap, errors::Handler, sync::Lrc};
use swc_ecma_ast::Module;
use swc_ecma_parser::{Parser, StringInput, Syntax, lexer::Lexer};

/// Parse a JavaScript or TypeScript file into an AST
pub fn parse_file(
    file_path: &Path,
    source_map: &Lrc<SourceMap>,
    handler: &Handler,
) -> Option<Module> {
    // Determine syntax based on file extension
    let syntax = if let Some(ext) = file_path.extension() {
        match ext.to_string_lossy().as_ref() {
            "ts" | "tsx" => Syntax::Typescript(Default::default()),
            _ => Syntax::Es(Default::default()),
        }
    } else {
        Syntax::Es(Default::default())
    };

    // Read file content
    let file_content = match fs::read_to_string(file_path) {
        Ok(content) => content,
        Err(e) => {
            eprintln!("Error reading file {}: {}", file_path.display(), e);
            return None;
        }
    };

    // Create source file in the source map
    let source_file = source_map.new_source_file(
        // Create an Lrc wrapped FileName
        Lrc::new(FileName::Real(file_path.to_path_buf())),
        file_content,
    );

    // Create lexer and parser
    let lexer = Lexer::new(
        syntax,
        Default::default(),
        StringInput::from(&*source_file),
        None,
    );
    let mut parser = Parser::new_from(lexer);

    // Parse module
    for e in parser.take_errors() {
        e.into_diagnostic(handler).emit();
    }

    match parser.parse_module() {
        Ok(module) => Some(module),
        Err(e) => {
            e.into_diagnostic(handler).emit();
            None
        }
    }
}
