//! Matching byte buffers against discriminator nodes.
//!
//! Rust counterpart of the TypeScript `dynamic-parsers/discriminators`.

use codama_dynamic_codecs::{get_node_codec, get_type_codec, get_value};
use codama_nodes::{
    ConstantDiscriminatorNode, ConstantValueNode, DiscriminatorNode, FieldDiscriminatorNode,
    RootNode, StructTypeNode,
};
use codama_visitors_core::NodePath;
use codama_visitors_core::{DynamicError as CodamaError, DynamicResult as CodamaResult};

/// How many discriminators whose type links elsewhere one identification may
/// read. Each copies the whole tree, and an IDL could give one to each of
/// thousands of instructions.
const MAX_LINKED_DISCRIMINATORS: usize = 64;

/// What one identification has spent on discriminators so far.
#[derive(Default)]
pub(crate) struct LinkedDiscriminators(usize);

/// Whether `bytes` match *every* discriminator. Returns `false` when there are
/// no discriminators (an undiscriminated node can never be identified by bytes).
///
/// `struct_type` provides the field definitions used by field discriminators;
/// constant and size discriminators ignore it.
pub(crate) fn match_discriminators(
    bytes: &[u8],
    discriminators: &[DiscriminatorNode],
    struct_type: Option<&StructTypeNode>,
    root: &RootNode,
    linked: &mut LinkedDiscriminators,
) -> CodamaResult<bool> {
    if discriminators.is_empty() {
        return Ok(false);
    }
    for discriminator in discriminators {
        if !match_discriminator(bytes, discriminator, struct_type, root, linked)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn match_discriminator(
    bytes: &[u8],
    discriminator: &DiscriminatorNode,
    struct_type: Option<&StructTypeNode>,
    root: &RootNode,
    linked: &mut LinkedDiscriminators,
) -> CodamaResult<bool> {
    match discriminator {
        DiscriminatorNode::Constant(node) => {
            match_constant_discriminator(bytes, node, root, linked)
        }
        DiscriminatorNode::Field(node) => {
            match_field_discriminator(bytes, node, struct_type, root, linked)
        }
        DiscriminatorNode::Size(node) => Ok(bytes.len() as u64 == node.size),
    }
}

fn match_constant_discriminator(
    bytes: &[u8],
    discriminator: &ConstantDiscriminatorNode,
    root: &RootNode,
    linked: &mut LinkedDiscriminators,
) -> CodamaResult<bool> {
    let to_match = encode_constant(root, &discriminator.constant, linked)?;
    Ok(contains_bytes(
        bytes,
        &to_match,
        discriminator.offset as usize,
    ))
}

fn match_field_discriminator(
    bytes: &[u8],
    discriminator: &FieldDiscriminatorNode,
    struct_type: Option<&StructTypeNode>,
    root: &RootNode,
    linked: &mut LinkedDiscriminators,
) -> CodamaResult<bool> {
    let constant = field_discriminator_to_constant(discriminator, struct_type)?;
    match_constant_discriminator(bytes, &constant, root, linked)
}

/// Resolves a field discriminator to the equivalent constant discriminator, by
/// looking up the named field's type and default value in `struct_type`.
fn field_discriminator_to_constant(
    discriminator: &FieldDiscriminatorNode,
    struct_type: Option<&StructTypeNode>,
) -> CodamaResult<ConstantDiscriminatorNode> {
    let struct_type = struct_type
        .ok_or_else(|| CodamaError::DiscriminatorFieldNotFound(discriminator.name.to_string()))?;
    let field = struct_type
        .fields
        .iter()
        .find(|field| field.name.as_str() == discriminator.name.as_str())
        .ok_or_else(|| CodamaError::DiscriminatorFieldNotFound(discriminator.name.to_string()))?;
    let default_value = field.default_value.as_ref().clone().ok_or_else(|| {
        CodamaError::DiscriminatorFieldHasNoDefaultValue(discriminator.name.to_string())
    })?;
    let constant = ConstantValueNode::new((*field.r#type).clone(), default_value);
    Ok(ConstantDiscriminatorNode::new(
        constant,
        discriminator.offset,
    ))
}

/// Encodes a constant value to its bytes, resolving any defined-type links in
/// the constant's type against the program at the head of the tree.
fn encode_constant(
    root: &RootNode,
    constant: &ConstantValueNode,
    linked: &mut LinkedDiscriminators,
) -> CodamaResult<Vec<u8>> {
    let value = get_value(constant.value.as_ref())?;
    // A discriminator is nearly always plain bytes or a number. Only a type
    // that links elsewhere needs the tree, and a path to it copies the tree.
    if let Ok(codec) = get_type_codec(&constant.r#type) {
        return codec.encode_to_vec(&value);
    }
    linked.0 += 1;
    if linked.0 > MAX_LINKED_DISCRIMINATORS {
        return Err(CodamaError::TypeTooLarge(MAX_LINKED_DISCRIMINATORS));
    }
    let path = NodePath::new()
        .extended(root.clone())
        .extended(root.program.clone())
        .extended((*constant.r#type).clone());
    let codec = get_node_codec(&path)?;
    codec.encode_to_vec(&value)
}

/// A byte-level filter derived from a node's discriminators, suitable for a
/// `getProgramAccounts` request: a `memcmp` on a byte slice, or a data-length
/// match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataFilter {
    /// Match `bytes` starting at `offset` in the account data.
    Memcmp { offset: usize, bytes: Vec<u8> },
    /// Match accounts whose data length equals `size`.
    DataSize(usize),
}

/// Computes the [`DataFilter`]s equivalent to a set of `discriminators`, so
/// on-chain accounts can be narrowed server-side before decoding.
///
/// `struct_type` provides the field definitions used by field discriminators;
/// constant and size discriminators ignore it.
pub fn account_data_filters(
    root: &RootNode,
    discriminators: &[DiscriminatorNode],
    struct_type: Option<&StructTypeNode>,
) -> CodamaResult<Vec<DataFilter>> {
    discriminators
        .iter()
        .map(|discriminator| discriminator_to_filter(root, discriminator, struct_type))
        .collect()
}

fn discriminator_to_filter(
    root: &RootNode,
    discriminator: &DiscriminatorNode,
    struct_type: Option<&StructTypeNode>,
) -> CodamaResult<DataFilter> {
    match discriminator {
        DiscriminatorNode::Constant(node) => Ok(DataFilter::Memcmp {
            offset: node.offset as usize,
            bytes: encode_constant(root, &node.constant, &mut LinkedDiscriminators::default())?,
        }),
        DiscriminatorNode::Field(node) => {
            let constant = field_discriminator_to_constant(node, struct_type)?;
            Ok(DataFilter::Memcmp {
                offset: constant.offset as usize,
                bytes: encode_constant(
                    root,
                    &constant.constant,
                    &mut LinkedDiscriminators::default(),
                )?,
            })
        }
        DiscriminatorNode::Size(node) => Ok(DataFilter::DataSize(node.size as usize)),
    }
}

/// Whether `data` contains `bytes` starting at `offset`. Mirrors
/// `@solana/codecs`' `containsBytes`: out-of-range offsets yield `false`, and an
/// empty `bytes` at the very end of `data` matches.
fn contains_bytes(data: &[u8], bytes: &[u8], offset: usize) -> bool {
    match data.get(offset..offset.saturating_add(bytes.len())) {
        Some(slice) => slice == bytes,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codama_nodes::{NumberTypeNode, NumberValueNode, ProgramNode, SizeDiscriminatorNode, U8};

    fn root() -> RootNode {
        RootNode::new(ProgramNode::new("p", "11111111111111111111111111111111"))
    }

    #[test]
    fn constant_becomes_memcmp_and_size_becomes_data_size() {
        let discriminators = vec![
            ConstantDiscriminatorNode::new(
                ConstantValueNode::new(NumberTypeNode::le(U8), NumberValueNode::new(7u8)),
                4,
            )
            .into(),
            SizeDiscriminatorNode::new(40).into(),
        ];
        let filters = account_data_filters(&root(), &discriminators, None).unwrap();
        assert_eq!(
            filters,
            vec![
                DataFilter::Memcmp {
                    offset: 4,
                    bytes: vec![7],
                },
                DataFilter::DataSize(40),
            ]
        );
    }
}
