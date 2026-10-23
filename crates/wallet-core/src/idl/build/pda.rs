//! Deriving an account address from the seeds its IDL names.

use super::*;

pub(super) fn apply_pda(
    field: &mut IdlAccountField,
    root: &RootNode,
    program_id: &Pubkey,
    node: &PdaValueNode,
    filled: &HashMap<String, Pubkey>,
    args: &HashMap<String, String>,
) {
    let pda = match node.pda.as_ref() {
        PdaValuePda::Pda(pda) => pda,
        PdaValuePda::PdaLink(link) => match find_pda(root, link.name.as_str()) {
            Some(pda) => pda,
            None => {
                field.source = IdlAccountSource::Fill;
                field.reason = format!("PDA `{}` is not in this IDL", link.name.as_str());
                return;
            }
        },
    };
    field.seeds = seed_notes(pda, node);
    let program = match pda_program(node, pda, program_id, filled, args) {
        ProgramPick::Ready(pk) => pk,
        ProgramPick::Waiting => {
            field.source = IdlAccountSource::Derived;
            return;
        }
        ProgramPick::Failed(reason) => {
            field.source = IdlAccountSource::Fill;
            field.reason = reason;
            return;
        }
    };
    let mut encoded = Vec::with_capacity(pda.seeds.len());
    for seed in &pda.seeds {
        match encode_seed(seed, node, program_id, filled, args) {
            SeedByte::Ready(bytes) => encoded.push(bytes),
            SeedByte::Waiting => {
                field.source = IdlAccountSource::Derived;
                return;
            }
            SeedByte::Failed(reason) => {
                field.source = IdlAccountSource::Fill;
                field.reason = reason;
                return;
            }
        }
    }
    let refs: Vec<&[u8]> = encoded.iter().map(Vec::as_slice).collect();
    let (address, _) = Pubkey::find_program_address(&refs, &program);
    field.source = IdlAccountSource::Derived;
    field.address = Some(address);
}

pub(super) enum ProgramPick {
    Ready(Pubkey),
    Waiting,
    Failed(String),
}

pub(super) fn pda_program(
    value: &PdaValueNode,
    pda: &codama_nodes::PdaNode,
    program_id: &Pubkey,
    filled: &HashMap<String, Pubkey>,
    args: &HashMap<String, String>,
) -> ProgramPick {
    match value.program_id.as_ref().as_ref() {
        Some(PdaValueProgramId::Account(account)) => match filled.get(account.name.as_str()) {
            Some(pk) => ProgramPick::Ready(*pk),
            None => ProgramPick::Waiting,
        },
        Some(PdaValueProgramId::Argument(argument)) => {
            match args.get(argument.name.as_str()).map(|text| text.trim()) {
                Some(text) if !text.is_empty() => match Pubkey::from_str(text) {
                    Ok(pk) => ProgramPick::Ready(pk),
                    Err(_) => ProgramPick::Failed(format!(
                        "program id argument `{}` is not an address",
                        argument.name.as_str()
                    )),
                },
                _ => ProgramPick::Waiting,
            }
        }
        None => match &pda.program_id {
            Some(text) => match Pubkey::from_str(text) {
                Ok(pk) => ProgramPick::Ready(pk),
                Err(_) => ProgramPick::Failed("program id is not an address".to_string()),
            },
            None => ProgramPick::Ready(*program_id),
        },
    }
}

pub(super) fn seed_notes(pda: &codama_nodes::PdaNode, value: &PdaValueNode) -> Vec<String> {
    pda.seeds
        .iter()
        .map(|seed| match seed {
            PdaSeedNode::Variable(variable) => variable.name.to_string(),
            PdaSeedNode::Constant(constant) => constant_note(&constant.value),
        })
        .chain(value.seeds.iter().filter_map(|seed| {
            // A value seed that is not on the PDA node still belongs in the label.
            let listed = pda.seeds.iter().any(|node| match node {
                PdaSeedNode::Variable(variable) => variable.name.as_str() == seed.name.as_str(),
                PdaSeedNode::Constant(_) => false,
            });
            if listed {
                None
            } else {
                Some(seed.name.to_string())
            }
        }))
        .collect()
}

pub(super) fn constant_note(value: &ConstantPdaSeedValue) -> String {
    match value {
        ConstantPdaSeedValue::String(node) => node.string.clone(),
        ConstantPdaSeedValue::Number(node) => number_text(&node.number),
        ConstantPdaSeedValue::PublicKey(node) => node.public_key.clone(),
        ConstantPdaSeedValue::Boolean(node) => node.boolean.to_string(),
        ConstantPdaSeedValue::ProgramId(_) => "program id".to_string(),
        ConstantPdaSeedValue::Bytes(_) => "bytes".to_string(),
        _ => "constant".to_string(),
    }
}

pub(super) enum SeedByte {
    Ready(Vec<u8>),
    Waiting,
    Failed(String),
}

/// The bytes of one seed, encoded as the instruction builder encodes them:
/// with the seed's declared type, through the same codecs.
pub(super) fn encode_seed(
    seed: &PdaSeedNode,
    value: &PdaValueNode,
    program_id: &Pubkey,
    filled: &HashMap<String, Pubkey>,
    args: &HashMap<String, String>,
) -> SeedByte {
    let none = HashMap::new();
    let variable = match seed {
        PdaSeedNode::Constant(constant) => {
            return ready(encode_constant_seed(constant, &none, program_id));
        }
        PdaSeedNode::Variable(variable) => variable,
    };
    let name = variable.name.as_str();
    let Some(seed_value) = value.seeds.iter().find(|item| item.name.as_str() == name) else {
        return SeedByte::Failed(format!("seed `{name}` has no value"));
    };
    match seed_value.value.as_ref() {
        PdaSeedValueValue::Account(account) => match filled.get(account.name.as_str()) {
            Some(pk) => SeedByte::Ready(pk.to_bytes().to_vec()),
            None => SeedByte::Waiting,
        },
        PdaSeedValueValue::Argument(argument) => {
            let text = args
                .get(argument.name.as_str())
                .map_or("", |text| text.trim());
            if text.is_empty() {
                return SeedByte::Waiting;
            }
            let kind = classify(&variable.r#type);
            if matches!(kind, IdlArgKind::Raw { .. }) {
                return SeedByte::Failed(format!("seed `{name}` cannot be derived offline"));
            }
            match parse_argument(&kind, text, false) {
                Ok(Some(parsed)) => {
                    ready(get_type_codec(&variable.r#type).and_then(|c| c.encode_to_vec(&parsed)))
                }
                Ok(None) => SeedByte::Waiting,
                Err(reason) => SeedByte::Failed(reason),
            }
        }
        literal => match encode_seed_literal(&variable.r#type, literal, &none) {
            Some(encoded) => ready(encoded),
            None => SeedByte::Failed(format!("seed `{name}` cannot be derived offline")),
        },
    }
}

fn ready(encoded: Result<Vec<u8>, DynamicError>) -> SeedByte {
    match encoded {
        Ok(bytes) => SeedByte::Ready(bytes),
        Err(err) => SeedByte::Failed(err.to_string()),
    }
}

pub(super) fn number_text(number: &Number) -> String {
    match number {
        Number::UnsignedInteger(value) => value.to_string(),
        Number::SignedInteger(value) => value.to_string(),
        Number::Float(value) => value.to_string(),
    }
}

pub(super) fn find_pda<'a>(root: &'a RootNode, name: &str) -> Option<&'a codama_nodes::PdaNode> {
    std::iter::once(&root.program)
        .chain(root.additional_programs.iter())
        .flat_map(|program| program.pdas.iter())
        .find(|pda| pda.name.as_str() == name)
}
