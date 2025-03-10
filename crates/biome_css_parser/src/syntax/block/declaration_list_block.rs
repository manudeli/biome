use crate::parser::CssParser;
use crate::syntax::block::ParseBlockBody;
use crate::syntax::{is_at_declaration, DeclarationList};
use biome_css_syntax::CssSyntaxKind;
use biome_css_syntax::CssSyntaxKind::*;
use biome_parser::parse_lists::ParseNodeList;
use biome_parser::CompletedMarker;

#[inline]
pub(crate) fn parse_declaration_list_block(p: &mut CssParser) -> CompletedMarker {
    DeclarationListBlock.parse_block_body(p)
}
struct DeclarationListBlock;

impl ParseBlockBody for DeclarationListBlock {
    const BLOCK_KIND: CssSyntaxKind = CSS_DECLARATION_LIST_BLOCK;

    fn is_at_element(&self, p: &mut CssParser) -> bool {
        is_at_declaration(p)
    }

    fn parse_list(&mut self, p: &mut CssParser) {
        DeclarationList.parse_list(p);
    }
}
