//! Human-readable instruction display.
//!
//! Rust counterpart of `@codama/dynamic-instructions` display helpers: intent
//! sentences, fallback field lists, amount/date/duration/string formatting,
//! provide/inject, and the accounts an offline bundle must include.

use crate::ParsedInstruction;
use codama_dynamic_codecs::{get_node_codec, NumberValue, Value};
use codama_nodes::{
    AccountFieldValueNode, AmountNumberDisplayNode, AmountTypeNode, DateTimeTypeNode, DisplaySkip,
    EnumTypeNode, EnumVariantTypeNode, InjectableNumberValueNode, InjectableStringValueNode,
    InstructionAccountNode, InstructionArgumentNode, InstructionNode,
    InstructionRemainingAccountsNode, IsSigner, NestedTypeNodeTrait, Node, Number,
    NumberDisplayNode, NumberTypeNode, NumberValueNode, RegisteredContextualValueNode,
    RegisteredValueNode, RootNode, SolAmountTypeNode, StringTypeNode, StructTypeNode, TypeNode,
    ValueNode,
};
use codama_visitors_core::{LinkableDictionary, NodePath};
use serde::{Deserialize, Serialize};
use solana_instruction::AccountMeta;
use solana_pubkey::Pubkey;
use std::collections::{HashMap, HashSet};
use std::str::FromStr;

/// One labeled line in the fallback field list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayField {
    pub label: String,
    pub value: String,
}

/// Rendered instruction: a short intent, an optional filled sentence, and the
/// fallback fields. `interpolated_intent` is `None` when a placeholder cannot
/// be filled or an amount lost its scale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstructionDisplay {
    pub intent: String,
    pub interpolated_intent: Option<String>,
    /// Present exactly when `interpolated_intent` is. Joining text with each
    /// value's `primary` reproduces that sentence.
    pub spans: Option<Vec<IntentSpan>>,
    pub fields: Vec<DisplayField>,
}

/// One piece of an interpolated sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntentSpan {
    Text(String),
    Value(IntentValue),
}

/// A placeholder that resolved. `primary` is the text inside the sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntentValue {
    pub placeholder: String,
    pub primary: String,
    pub body: IntentBody,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntentBody {
    Address(Pubkey),
    Amount {
        raw: String,
        decimals: Option<u8>,
        unit: Option<String>,
        scale_from: ScaleSource,
    },
    Text(String),
}

/// Where an amount's decimals and unit were read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScaleSource {
    Instruction,
    Account(Pubkey),
    Missing,
}

/// Offline bundle: account bytes and optional labels, keyed by base58 address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DisplayDictionary {
    pub accounts: Vec<DictionaryAccount>,
    pub names: Vec<DictionaryName>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DictionaryAccount {
    pub address: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DictionaryName {
    pub address: String,
    pub name: String,
}

impl DisplayDictionary {
    pub fn account_map(&self) -> HashMap<Pubkey, Vec<u8>> {
        let mut map = HashMap::new();
        for account in &self.accounts {
            if let Ok(pk) = Pubkey::from_str(&account.address) {
                map.insert(pk, account.data.clone());
            }
        }
        map
    }

    pub fn name_map(&self) -> HashMap<Pubkey, String> {
        let mut map = HashMap::new();
        for name in &self.names {
            if let Ok(pk) = Pubkey::from_str(&name.address) {
                map.insert(pk, name.name.clone());
            }
        }
        map
    }
}

/// Renders `parsed` using account bytes already in hand. Missing keys mean the
/// account was not fetched; display then keeps the raw field visible.
pub fn get_instruction_display(
    root: &RootNode,
    parsed: &ParsedInstruction,
    accounts: &HashMap<Pubkey, Vec<u8>>,
    names: &HashMap<Pubkey, String>,
) -> Option<InstructionDisplay> {
    let instruction = match parsed.path.last()? {
        Node::Instruction(ix) => ix,
        _ => return None,
    };
    let linkables = LinkableDictionary::record_all(root);
    let provides = instruction
        .provides
        .iter()
        .map(|p| (p.name.as_str().to_string(), (*p.node).clone()))
        .collect();
    let ctx = Ctx {
        instruction,
        parsed,
        accounts,
        names,
        linkables: &linkables,
        provides: &provides,
    };
    let consumed = consumed_members(&ctx);

    let mut shown_args: HashMap<String, Shown> = HashMap::new();
    let mut fields = Vec::new();
    push_arguments(
        &instruction.arguments,
        &parsed.data,
        &ctx,
        &consumed,
        &mut shown_args,
        &mut fields,
    );
    push_accounts(&instruction.accounts, &ctx, &consumed, &mut fields);
    push_remaining(
        &instruction.remaining_accounts,
        &parsed.remaining_accounts,
        &ctx,
        &mut fields,
    );

    let intent = match instruction.display.as_ref().and_then(|d| d.intent.as_ref()) {
        Some(intent) => intent.clone(),
        None => title_case(instruction.name.as_str()),
    };
    let interpolated = instruction
        .display
        .as_ref()
        .and_then(|d| d.interpolated_intent.as_deref())
        .and_then(|template| interpolate(template, &shown_args, &ctx));
    let (interpolated_intent, spans) = match interpolated {
        Some((sentence, spans)) => (Some(sentence), Some(spans)),
        None => (None, None),
    };

    Some(InstructionDisplay {
        intent,
        interpolated_intent,
        spans,
        fields,
    })
}

pub fn get_instruction_display_from_dictionary(
    root: &RootNode,
    parsed: &ParsedInstruction,
    dictionary: &DisplayDictionary,
) -> Option<InstructionDisplay> {
    let accounts = dictionary.account_map();
    let names = dictionary.name_map();
    get_instruction_display(root, parsed, &accounts, &names)
}

/// Addresses an `accountField` injection would read. No account bytes required.
pub fn required_accounts_for_display(root: &RootNode, parsed: &ParsedInstruction) -> Vec<Pubkey> {
    let Some(Node::Instruction(instruction)) = parsed.path.last() else {
        return Vec::new();
    };
    let linkables = LinkableDictionary::record_all(root);
    let provides: HashMap<String, Node> = instruction
        .provides
        .iter()
        .map(|p| (p.name.as_str().to_string(), (*p.node).clone()))
        .collect();
    let ctx = Ctx {
        instruction,
        parsed,
        accounts: &HashMap::new(),
        names: &HashMap::new(),
        linkables: &linkables,
        provides: &provides,
    };
    let mut out = Vec::new();
    let mut seen_pk = HashSet::new();
    for node in collect_injections(&ctx) {
        let mut seen = HashSet::new();
        let Some(terminal) = terminal_node(&node, &provides, &mut seen) else {
            continue;
        };
        let Node::ContextualValue(RegisteredContextualValueNode::AccountField(field)) = &terminal
        else {
            continue;
        };
        let Some(pk) = find_account(parsed, field.account.as_str()) else {
            continue;
        };
        if seen_pk.insert(pk) {
            out.push(pk);
        }
    }
    out
}

struct Shown {
    text: String,
    degraded: bool,
    detail: SpanDetail,
}

#[derive(Clone)]
enum SpanDetail {
    Text,
    Address(Pubkey),
    Amount {
        raw: String,
        decimals: Option<u8>,
        unit: Option<String>,
        scale_from: ScaleSource,
    },
}

struct Ctx<'a> {
    instruction: &'a InstructionNode,
    parsed: &'a ParsedInstruction,
    accounts: &'a HashMap<Pubkey, Vec<u8>>,
    names: &'a HashMap<Pubkey, String>,
    linkables: &'a LinkableDictionary,
    provides: &'a HashMap<String, Node>,
}

fn consumed_members(ctx: &Ctx<'_>) -> HashSet<String> {
    let mut consumed = HashSet::new();
    for node in collect_injections(ctx) {
        let mut names = HashSet::new();
        if resolve_node(&node, ctx, &mut HashSet::new(), &mut names).is_some() {
            consumed.extend(names);
        }
    }
    consumed
}

fn collect_injections(ctx: &Ctx<'_>) -> Vec<Node> {
    let mut out = Vec::new();
    for arg in &ctx.instruction.arguments {
        if skip_always(arg.display.as_ref().and_then(|d| d.skip)) {
            continue;
        }
        let flatten = arg
            .display
            .as_ref()
            .and_then(|d| d.flatten)
            .unwrap_or(false);
        collect_type_injections(arg.r#type.as_ref(), ctx, flatten, &mut out);
    }
    out
}

fn collect_type_injections(ty: &TypeNode, ctx: &Ctx<'_>, enter_struct: bool, out: &mut Vec<Node>) {
    let resolved = display_type(ty, ctx.linkables, &ctx.parsed.path);
    match &resolved {
        TypeNode::Number(n) => push_amount_injections(n, out),
        TypeNode::Amount(a) => push_amount_injections(a.number.get_nested_type_node(), out),
        TypeNode::SolAmount(a) => push_amount_injections(a.number.get_nested_type_node(), out),
        TypeNode::DateTime(a) => push_amount_injections(a.number.get_nested_type_node(), out),
        TypeNode::Struct(s) if enter_struct => {
            for field in &s.fields {
                if skip_always(field.display.as_ref().and_then(|d| d.skip)) {
                    continue;
                }
                let flatten = field
                    .display
                    .as_ref()
                    .and_then(|d| d.flatten)
                    .unwrap_or(false);
                collect_type_injections(field.r#type.as_ref(), ctx, flatten, out);
            }
        }
        _ => {}
    }
}

fn push_amount_injections(number: &NumberTypeNode, out: &mut Vec<Node>) {
    let Some(NumberDisplayNode::Amount(amount)) = number.display.as_ref().as_ref() else {
        return;
    };
    if let Some(InjectableNumberValueNode::Injected(node)) = amount.decimals.as_ref().as_ref() {
        out.push(node.clone().into());
    }
    if let Some(InjectableStringValueNode::Injected(node)) = amount.unit.as_ref().as_ref() {
        out.push(node.clone().into());
    }
}

fn push_arguments(
    args: &[InstructionArgumentNode],
    data: &Value,
    ctx: &Ctx<'_>,
    consumed: &HashSet<String>,
    shown: &mut HashMap<String, Shown>,
    fields: &mut Vec<DisplayField>,
) {
    let Value::Struct(map) = data else {
        return;
    };
    for arg in args {
        let Some(value) = map.get(arg.name.as_str()) else {
            continue;
        };
        let formatted = format_value(value, arg.r#type.as_ref(), ctx);
        let hide = member_hidden(
            arg.display.as_ref().map(|d| (d.label.as_deref(), d.skip)),
            arg.name.as_str(),
            consumed,
        );
        shown.insert(arg.name.as_str().to_string(), formatted_clone(&formatted));
        if hide {
            continue;
        }
        let label = member_label(
            arg.display.as_ref().and_then(|d| d.label.as_deref()),
            arg.name.as_str(),
        );
        let flatten = arg
            .display
            .as_ref()
            .and_then(|d| d.flatten)
            .unwrap_or(false);
        let prefix = arg
            .display
            .as_ref()
            .and_then(|d| d.flatten_prefix.clone())
            .unwrap_or_default();
        if flatten
            && push_flat_struct(
                value,
                arg.r#type.as_ref(),
                ctx,
                consumed,
                arg.name.as_str(),
                &prefix,
                fields,
            )
        {
            continue;
        }
        if push_pubkey_array(value, arg.r#type.as_ref(), ctx, &label, fields) {
            continue;
        }
        fields.push(DisplayField {
            label,
            value: formatted.text,
        });
    }
}

fn formatted_clone(shown: &Shown) -> Shown {
    Shown {
        text: shown.text.clone(),
        degraded: shown.degraded,
        detail: shown.detail.clone(),
    }
}

fn push_flat_struct(
    value: &Value,
    ty: &TypeNode,
    ctx: &Ctx<'_>,
    consumed: &HashSet<String>,
    path: &str,
    prefix: &str,
    fields: &mut Vec<DisplayField>,
) -> bool {
    let TypeNode::Struct(s) = display_type(ty, ctx.linkables, &ctx.parsed.path) else {
        return false;
    };
    let Value::Struct(map) = value else {
        return false;
    };
    push_struct_fields(&s, map, ctx, consumed, path, prefix, fields);
    true
}

fn push_struct_fields(
    s: &StructTypeNode,
    map: &indexmap::IndexMap<String, Value>,
    ctx: &Ctx<'_>,
    consumed: &HashSet<String>,
    path: &str,
    prefix: &str,
    fields: &mut Vec<DisplayField>,
) {
    for field in &s.fields {
        let Some(value) = map.get(field.name.as_str()) else {
            continue;
        };
        // Injections name nested fields by their dotted path from the
        // instruction data, so `whenInjected` matches on that path.
        let field_path = format!("{path}.{}", field.name.as_str());
        if member_hidden(
            field.display.as_ref().map(|d| (d.label.as_deref(), d.skip)),
            &field_path,
            consumed,
        ) {
            continue;
        }
        let label = format!(
            "{prefix}{}",
            member_label(
                field.display.as_ref().and_then(|d| d.label.as_deref()),
                field.name.as_str(),
            )
        );
        let nested_flatten = field
            .display
            .as_ref()
            .and_then(|d| d.flatten)
            .unwrap_or(false);
        let nested_prefix = field
            .display
            .as_ref()
            .and_then(|d| d.flatten_prefix.clone())
            .unwrap_or_default();
        if nested_flatten
            && push_flat_struct(
                value,
                field.r#type.as_ref(),
                ctx,
                consumed,
                &field_path,
                &format!("{prefix}{nested_prefix}"),
                fields,
            )
        {
            continue;
        }
        let formatted = format_value(value, field.r#type.as_ref(), ctx);
        fields.push(DisplayField {
            label,
            value: formatted.text,
        });
    }
}

fn push_pubkey_array(
    value: &Value,
    ty: &TypeNode,
    ctx: &Ctx<'_>,
    label: &str,
    fields: &mut Vec<DisplayField>,
) -> bool {
    let resolved = display_type(ty, ctx.linkables, &ctx.parsed.path);
    let TypeNode::Array(array) = &resolved else {
        return false;
    };
    if !matches!(
        display_type(array.item.as_ref(), ctx.linkables, &ctx.parsed.path),
        TypeNode::PublicKey(_)
    ) {
        return false;
    }
    let Value::Array(items) = value else {
        return false;
    };
    let keys: Vec<Pubkey> = items.iter().filter_map(value_pubkey).collect();
    if keys.len() != items.len() || keys.len() <= 1 {
        return false;
    }
    for (i, pk) in keys.iter().enumerate() {
        fields.push(DisplayField {
            label: format!("{label} #{}", i + 1),
            value: pk.to_string(),
        });
    }
    true
}

fn push_accounts(
    accounts: &[InstructionAccountNode],
    ctx: &Ctx<'_>,
    consumed: &HashSet<String>,
    fields: &mut Vec<DisplayField>,
) {
    for account in accounts {
        if member_hidden(
            account
                .display
                .as_ref()
                .map(|d| (d.label.as_deref(), d.skip)),
            account.name.as_str(),
            consumed,
        ) {
            continue;
        }
        let Some(pk) = find_account(ctx.parsed, account.name.as_str()) else {
            continue;
        };
        fields.push(DisplayField {
            label: member_label(
                account.display.as_ref().and_then(|d| d.label.as_deref()),
                account.name.as_str(),
            ),
            value: named_address(pk, ctx.names),
        });
    }
}

fn push_remaining(
    groups: &[InstructionRemainingAccountsNode],
    metas: &[AccountMeta],
    ctx: &Ctx<'_>,
    fields: &mut Vec<DisplayField>,
) {
    let mut rest: Vec<AccountMeta> = metas.to_vec();
    for (index, group) in groups.iter().enumerate() {
        let last = index + 1 == groups.len();
        let taken = if last {
            std::mem::take(&mut rest)
        } else {
            let mut taken = Vec::new();
            let mut kept = Vec::new();
            for meta in rest.drain(..) {
                if signer_matches(group.is_signer, meta.is_signer) {
                    taken.push(meta);
                } else {
                    kept.push(meta);
                }
            }
            rest = kept;
            taken
        };
        if skip_always(group.display.as_ref().and_then(|d| d.skip)) || taken.is_empty() {
            continue;
        }
        let label = member_label(
            group.display.as_ref().and_then(|d| d.label.as_deref()),
            "remaining",
        );
        if taken.len() == 1 {
            fields.push(DisplayField {
                label,
                value: named_address(taken[0].pubkey, ctx.names),
            });
        } else {
            for (i, meta) in taken.iter().enumerate() {
                fields.push(DisplayField {
                    label: format!("{label} #{}", i + 1),
                    value: named_address(meta.pubkey, ctx.names),
                });
            }
        }
    }
}

fn signer_matches(expected: Option<IsSigner>, is_signer: bool) -> bool {
    match expected {
        Some(IsSigner::True) => is_signer,
        Some(IsSigner::False) => !is_signer,
        Some(IsSigner::Either) | None => true,
    }
}

fn interpolate(
    template: &str,
    args: &HashMap<String, Shown>,
    ctx: &Ctx<'_>,
) -> Option<(String, Vec<IntentSpan>)> {
    let mut out = String::new();
    let mut spans = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find("${") {
        push_text(&mut out, &mut spans, &rest[..start]);
        rest = &rest[start + 2..];
        let end = rest.find('}')?;
        let body = rest[..end].trim();
        rest = &rest[end + 1..];
        let (kind, name) = body.split_once('.')?;
        let kind = kind.trim();
        let name = name.trim();
        let (text, intent_body) = match kind {
            "data" => {
                let nested;
                let shown = match args.get(name) {
                    Some(shown) => shown,
                    None => {
                        nested = nested_argument(name, ctx)?;
                        &nested
                    }
                };
                if shown.degraded {
                    return None;
                }
                (shown.text.clone(), intent_body(shown))
            }
            "accounts" => {
                let pk = find_account(ctx.parsed, name)?;
                (pk.to_string(), IntentBody::Address(pk))
            }
            _ => return None,
        };
        out.push_str(&text);
        spans.push(IntentSpan::Value(IntentValue {
            placeholder: format!("{kind}.{name}"),
            primary: text,
            body: intent_body,
        }));
    }
    push_text(&mut out, &mut spans, rest);
    debug_assert_eq!(join_spans(&spans), out);
    Some((out, spans))
}

/// A field inside an argument, by its dotted path from the instruction data
/// (`args.amount`), formatted with the field's own type.
fn nested_argument(path: &str, ctx: &Ctx<'_>) -> Option<Shown> {
    let (first, rest) = path.split_once('.')?;
    let arg = ctx
        .instruction
        .arguments
        .iter()
        .find(|arg| arg.name.as_str() == first)?;
    let mut value = struct_field(&ctx.parsed.data, first)?;
    let mut ty = arg.r#type.as_ref().clone();
    for part in rest.split('.') {
        let TypeNode::Struct(s) = display_type(&ty, ctx.linkables, &ctx.parsed.path) else {
            return None;
        };
        let field = s.fields.iter().find(|f| f.name.as_str() == part)?;
        value = struct_field(unwrap_some(value), part)?;
        ty = field.r#type.as_ref().clone();
    }
    Some(format_value(value, &ty, ctx))
}

fn unwrap_some(value: &Value) -> &Value {
    match value {
        Value::Option(Some(inner)) => unwrap_some(inner),
        other => other,
    }
}

fn push_text(out: &mut String, spans: &mut Vec<IntentSpan>, text: &str) {
    if text.is_empty() {
        return;
    }
    out.push_str(text);
    spans.push(IntentSpan::Text(text.to_string()));
}

fn join_spans(spans: &[IntentSpan]) -> String {
    let mut out = String::new();
    for span in spans {
        match span {
            IntentSpan::Text(text) => out.push_str(text),
            IntentSpan::Value(value) => out.push_str(&value.primary),
        }
    }
    out
}

fn intent_body(shown: &Shown) -> IntentBody {
    match &shown.detail {
        SpanDetail::Address(pk) => IntentBody::Address(*pk),
        SpanDetail::Amount {
            raw,
            decimals,
            unit,
            scale_from,
        } => IntentBody::Amount {
            raw: raw.clone(),
            decimals: *decimals,
            unit: unit.clone(),
            scale_from: scale_from.clone(),
        },
        SpanDetail::Text => IntentBody::Text(shown.text.clone()),
    }
}

fn format_value(value: &Value, ty: &TypeNode, ctx: &Ctx<'_>) -> Shown {
    match value {
        Value::Option(None) => plain("None"),
        Value::Option(Some(inner)) => format_value(inner, ty, ctx),
        other => {
            let resolved = display_type(ty, ctx.linkables, &ctx.parsed.path);
            format_concrete(other, &resolved, ctx)
        }
    }
}

fn format_concrete(value: &Value, ty: &TypeNode, ctx: &Ctx<'_>) -> Shown {
    match ty {
        TypeNode::Number(n) => match n.display.as_ref().as_ref() {
            Some(display) => format_number_display(value, display, ctx),
            None => plain(&default_text(value)),
        },
        TypeNode::PublicKey(_) => match value_pubkey(value) {
            Some(pk) => address_shown(pk),
            None => plain(&default_text(value)),
        },
        TypeNode::Amount(amount) => format_amount_type(value, amount, ctx),
        TypeNode::SolAmount(amount) => format_sol_amount(value, amount, ctx),
        TypeNode::DateTime(amount) => format_date_type(value, amount, ctx),
        TypeNode::String(s) => format_string(value, s),
        TypeNode::Enum(e) => format_enum(value, e, ctx),
        TypeNode::Array(_)
        | TypeNode::Struct(_)
        | TypeNode::Tuple(_)
        | TypeNode::Map(_)
        | TypeNode::Set(_) => plain(&default_text(value)),
        _ => plain(&default_text(value)),
    }
}

fn format_amount_type(value: &Value, amount: &AmountTypeNode, ctx: &Ctx<'_>) -> Shown {
    let inner = amount.number.get_nested_type_node();
    if let Some(display) = inner.display.as_ref().as_ref() {
        return format_number_display(value, display, ctx);
    }
    scale_shown(
        value,
        amount.decimals,
        amount.unit.as_deref(),
        ScaleSource::Instruction,
    )
}

fn format_sol_amount(value: &Value, amount: &SolAmountTypeNode, ctx: &Ctx<'_>) -> Shown {
    let inner = amount.number.get_nested_type_node();
    if let Some(display) = inner.display.as_ref().as_ref() {
        return format_number_display(value, display, ctx);
    }
    scale_shown(value, 9, Some("SOL"), ScaleSource::Instruction)
}

fn format_date_type(value: &Value, amount: &DateTimeTypeNode, ctx: &Ctx<'_>) -> Shown {
    let inner = amount.number.get_nested_type_node();
    if let Some(display) = inner.display.as_ref().as_ref() {
        return format_number_display(value, display, ctx);
    }
    match format_datetime(value, None) {
        Some(text) => plain(&text),
        None => plain(&default_text(value)),
    }
}

fn format_number_display(value: &Value, display: &NumberDisplayNode, ctx: &Ctx<'_>) -> Shown {
    match display {
        NumberDisplayNode::Amount(amount) => format_amount_display(value, amount, ctx),
        NumberDisplayNode::DateTime(date) => match format_datetime(value, date.ticks_per_second) {
            Some(text) => plain(&text),
            None => plain(&default_text(value)),
        },
        NumberDisplayNode::Duration(duration) => {
            match format_duration(value, duration.ticks_per_second) {
                Some(text) => plain(&text),
                None => plain(&default_text(value)),
            }
        }
    }
}

fn format_amount_display(value: &Value, amount: &AmountNumberDisplayNode, ctx: &Ctx<'_>) -> Shown {
    let Some(decimals) = resolve_decimals(amount, ctx) else {
        return degraded(value);
    };
    let Some(unit) = resolve_unit(amount, ctx) else {
        return degraded(value);
    };
    let scale_from = match (
        number_scale_source(amount.decimals.as_ref().as_ref(), ctx),
        string_scale_source(amount.unit.as_ref().as_ref(), ctx),
    ) {
        (ScaleSource::Account(pk), _) | (_, ScaleSource::Account(pk)) => ScaleSource::Account(pk),
        _ => ScaleSource::Instruction,
    };
    scale_shown(value, decimals, unit.as_deref(), scale_from)
}

fn number_scale_source(node: Option<&InjectableNumberValueNode>, ctx: &Ctx<'_>) -> ScaleSource {
    match node {
        Some(InjectableNumberValueNode::Injected(injected)) => ctx
            .provides
            .get(injected.key.as_str())
            .map(|provided| account_scale_source(provided, ctx))
            .unwrap_or(ScaleSource::Instruction),
        _ => ScaleSource::Instruction,
    }
}

fn string_scale_source(node: Option<&InjectableStringValueNode>, ctx: &Ctx<'_>) -> ScaleSource {
    match node {
        Some(InjectableStringValueNode::Injected(injected)) => ctx
            .provides
            .get(injected.key.as_str())
            .map(|provided| account_scale_source(provided, ctx))
            .unwrap_or(ScaleSource::Instruction),
        _ => ScaleSource::Instruction,
    }
}

fn account_scale_source(node: &Node, ctx: &Ctx<'_>) -> ScaleSource {
    let mut seen = HashSet::new();
    match terminal_node(node, ctx.provides, &mut seen) {
        Some(Node::ContextualValue(RegisteredContextualValueNode::AccountField(field))) => {
            find_account(ctx.parsed, field.account.as_str())
                .map(ScaleSource::Account)
                .unwrap_or(ScaleSource::Missing)
        }
        _ => ScaleSource::Instruction,
    }
}

fn scale_shown(value: &Value, decimals: u32, unit: Option<&str>, scale_from: ScaleSource) -> Shown {
    let Some(scaled) = scale_value(value, decimals) else {
        return degraded(value);
    };
    let text = match unit {
        Some(unit) if !unit.is_empty() => format!("{scaled} {unit}"),
        _ => scaled,
    };
    let raw = raw_integer(value).unwrap_or_else(|| default_text(value));
    Shown {
        text,
        degraded: false,
        detail: SpanDetail::Amount {
            raw,
            decimals: u8::try_from(decimals).ok(),
            unit: unit.filter(|unit| !unit.is_empty()).map(str::to_string),
            scale_from,
        },
    }
}

fn degraded(value: &Value) -> Shown {
    Shown {
        text: format!(
            "{} (raw)",
            raw_integer(value).unwrap_or_else(|| default_text(value))
        ),
        degraded: true,
        detail: SpanDetail::Text,
    }
}

fn plain(text: &str) -> Shown {
    Shown {
        text: text.to_string(),
        degraded: false,
        detail: SpanDetail::Text,
    }
}

fn address_shown(pk: Pubkey) -> Shown {
    Shown {
        text: pk.to_string(),
        degraded: false,
        detail: SpanDetail::Address(pk),
    }
}

fn resolve_decimals(amount: &AmountNumberDisplayNode, ctx: &Ctx<'_>) -> Option<u32> {
    match amount.decimals.as_ref().as_ref() {
        None => Some(0),
        Some(InjectableNumberValueNode::Number(n)) => number_node_u32(n),
        Some(InjectableNumberValueNode::Injected(n)) => {
            let value = resolve_node(
                &n.clone().into(),
                ctx,
                &mut HashSet::new(),
                &mut HashSet::new(),
            )?;
            value_u32(&value)
        }
    }
}

fn resolve_unit(amount: &AmountNumberDisplayNode, ctx: &Ctx<'_>) -> Option<Option<String>> {
    match amount.unit.as_ref().as_ref() {
        None => Some(None),
        Some(InjectableStringValueNode::String(s)) => Some(Some(s.string.clone())),
        Some(InjectableStringValueNode::Injected(n)) => {
            let value = resolve_node(
                &n.clone().into(),
                ctx,
                &mut HashSet::new(),
                &mut HashSet::new(),
            )?;
            match value {
                Value::String(s) => Some(Some(s)),
                _ => None,
            }
        }
    }
}

fn format_string(value: &Value, ty: &StringTypeNode) -> Shown {
    let Value::String(s) = value else {
        return plain(&default_text(value));
    };
    let Some(display) = &ty.display else {
        return plain(s);
    };
    plain(&slice_chars(s, display.slice_start, display.slice_end))
}

fn format_enum(value: &Value, ty: &EnumTypeNode, ctx: &Ctx<'_>) -> Shown {
    let Some(index) = match_variant(ty, value) else {
        return plain(&default_text(value));
    };
    let variant = &ty.variants[index];
    let (label, skip_inner) = variant_label(variant);
    if skip_inner {
        return plain(&label);
    }
    let inner = enum_inner_text(value, variant, ctx);
    if inner.is_empty() {
        plain(&label)
    } else {
        plain(&format!("{label} ({inner})"))
    }
}

fn enum_inner_text(value: &Value, variant: &EnumVariantTypeNode, ctx: &Ctx<'_>) -> String {
    let Value::Enum { fields, .. } = value else {
        return String::new();
    };
    match (variant, fields.as_ref()) {
        (EnumVariantTypeNode::Struct(node), Value::Struct(map)) => {
            let mut parts = Vec::new();
            let struct_node = node.r#struct.get_nested_type_node();
            for field in &struct_node.fields {
                let Some(child) = map.get(field.name.as_str()) else {
                    continue;
                };
                let shown = format_value(child, field.r#type.as_ref(), ctx);
                parts.push(format!(
                    "{}: {}",
                    member_label(None, field.name.as_str()),
                    shown.text
                ));
            }
            parts.join(", ")
        }
        (EnumVariantTypeNode::Tuple(_), Value::Struct(map)) => match map.get("fields") {
            Some(Value::Tuple(items)) | Some(Value::Array(items)) => items
                .iter()
                .map(default_text)
                .collect::<Vec<_>>()
                .join(", "),
            _ => String::new(),
        },
        _ => String::new(),
    }
}

fn match_variant(ty: &EnumTypeNode, value: &Value) -> Option<usize> {
    match value {
        Value::Enum { variant, .. } => ty.variants.iter().position(|v| {
            let name = variant_name(v);
            name == *variant || pascal_case(&name) == *variant
        }),
        Value::String(s) => ty.variants.iter().position(|v| {
            let name = variant_name(v);
            name == *s || pascal_case(&name) == *s
        }),
        Value::Number(_) => {
            let n = value_u32(value)?;
            ty.variants
                .iter()
                .position(|v| match variant_disc(v) {
                    Some(disc) => disc == n,
                    None => false,
                })
                .or_else(|| {
                    ty.variants.iter().enumerate().find_map(|(i, v)| {
                        if variant_disc(v).is_none() && i as u32 == n {
                            Some(i)
                        } else {
                            None
                        }
                    })
                })
        }
        Value::Struct(map) => match map.get("__kind") {
            Some(Value::String(kind)) => ty.variants.iter().position(|v| {
                let name = variant_name(v);
                name == *kind || pascal_case(&name) == *kind
            }),
            _ => None,
        },
        _ => None,
    }
}

fn variant_name(variant: &EnumVariantTypeNode) -> String {
    match variant {
        EnumVariantTypeNode::Empty(n) => n.name.as_str().to_string(),
        EnumVariantTypeNode::Struct(n) => n.name.as_str().to_string(),
        EnumVariantTypeNode::Tuple(n) => n.name.as_str().to_string(),
    }
}

fn variant_disc(variant: &EnumVariantTypeNode) -> Option<u32> {
    match variant {
        EnumVariantTypeNode::Empty(n) => n.discriminator,
        EnumVariantTypeNode::Struct(n) => n.discriminator,
        EnumVariantTypeNode::Tuple(n) => n.discriminator,
    }
}

fn variant_label(variant: &EnumVariantTypeNode) -> (String, bool) {
    let display = match variant {
        EnumVariantTypeNode::Empty(n) => n.display.as_ref(),
        EnumVariantTypeNode::Struct(n) => n.display.as_ref(),
        EnumVariantTypeNode::Tuple(n) => n.display.as_ref(),
    };
    let name = variant_name(variant);
    let label = display
        .and_then(|d| d.label.clone())
        .unwrap_or_else(|| title_case(&name));
    let skip_inner = display.and_then(|d| d.skip_inner_data).unwrap_or(false);
    (label, skip_inner)
}

fn display_type(ty: &TypeNode, linkables: &LinkableDictionary, path: &NodePath) -> TypeNode {
    let mut ty = ty.clone();
    let mut path = path.clone();
    let mut seen = HashSet::new();
    loop {
        ty = peel_wrappers(ty);
        ty = match ty {
            TypeNode::Option(n) => *n.item,
            TypeNode::RemainderOption(n) => *n.item,
            TypeNode::ZeroableOption(n) => *n.item,
            TypeNode::Link(link) => {
                let name = link.name.as_str().to_string();
                if !seen.insert(name) {
                    return TypeNode::Link(link);
                }
                let link_path = path.extended(link.clone());
                let Some(def_path) = linkables.get_path(&link_path) else {
                    return TypeNode::Link(link);
                };
                let next = match def_path.last() {
                    Some(Node::DefinedType(def)) => (*def.r#type).clone(),
                    _ => return TypeNode::Link(link),
                };
                path = def_path;
                next
            }
            other => return other,
        };
    }
}

fn peel_wrappers(mut ty: TypeNode) -> TypeNode {
    loop {
        ty = match ty {
            TypeNode::FixedSize(n) => *n.r#type,
            TypeNode::SizePrefix(n) => *n.r#type,
            TypeNode::HiddenPrefix(n) => *n.r#type,
            TypeNode::HiddenSuffix(n) => *n.r#type,
            TypeNode::PreOffset(n) => *n.r#type,
            TypeNode::PostOffset(n) => *n.r#type,
            TypeNode::Sentinel(n) => *n.r#type,
            other => return other,
        };
    }
}

fn resolve_node(
    node: &Node,
    ctx: &Ctx<'_>,
    seen: &mut HashSet<String>,
    names: &mut HashSet<String>,
) -> Option<Value> {
    match node {
        Node::Value(RegisteredValueNode::Injected(injected)) => {
            let key = injected.key.as_str().to_string();
            if !seen.insert(key) {
                return None;
            }
            let next = if let Some(provided) = ctx.provides.get(injected.key.as_str()) {
                provided.clone()
            } else {
                injected
                    .fallback
                    .as_ref()
                    .as_ref()
                    .map(value_node_to_node)?
            };
            resolve_node(&next, ctx, seen, names)
        }
        Node::Value(RegisteredValueNode::Number(n)) => Some(number_from_node(n)),
        Node::Value(RegisteredValueNode::String(n)) => Some(Value::String(n.string.clone())),
        Node::Value(RegisteredValueNode::Boolean(n)) => Some(Value::Bool(n.boolean)),
        Node::ContextualValue(RegisteredContextualValueNode::Argument(n)) => {
            let value = argument_at(&ctx.parsed.data, n.name.as_str())?.clone();
            names.insert(n.name.as_str().to_string());
            Some(value)
        }
        Node::ContextualValue(RegisteredContextualValueNode::Account(n)) => {
            let pk = find_account(ctx.parsed, n.name.as_str())?;
            names.insert(n.name.as_str().to_string());
            Some(Value::PublicKey(pk.to_bytes()))
        }
        Node::ContextualValue(RegisteredContextualValueNode::AccountField(n)) => {
            let value = account_field_value(ctx, n)?;
            names.insert(n.account.as_str().to_string());
            Some(value)
        }
        _ => None,
    }
}

fn account_field_value(ctx: &Ctx<'_>, field: &AccountFieldValueNode) -> Option<Value> {
    let pk = find_account(ctx.parsed, field.account.as_str())?;
    let bytes = ctx.accounts.get(&pk)?;
    let path = field.path.as_ref()?.as_str();
    let decoded = decode_linked_account(ctx, field.account.as_str(), bytes)?;
    value_at(&decoded, path).cloned()
}

fn decode_linked_account(ctx: &Ctx<'_>, account_name: &str, bytes: &[u8]) -> Option<Value> {
    let account = ctx
        .instruction
        .accounts
        .iter()
        .find(|a| a.name.as_str() == account_name)?;
    let link = account.account_link.as_ref()?;
    let link_path = ctx.parsed.path.extended(link.clone());
    let account_path = ctx.linkables.get_path(&link_path)?;
    let codec = get_node_codec(&account_path).ok()?;
    let mut input = bytes;
    codec.decode(&mut input).ok()
}

fn terminal_node(
    node: &Node,
    provides: &HashMap<String, Node>,
    seen: &mut HashSet<String>,
) -> Option<Node> {
    match node {
        Node::Value(RegisteredValueNode::Injected(injected)) => {
            let key = injected.key.as_str().to_string();
            if !seen.insert(key) {
                return None;
            }
            let next = if let Some(provided) = provides.get(injected.key.as_str()) {
                provided.clone()
            } else {
                injected
                    .fallback
                    .as_ref()
                    .as_ref()
                    .map(value_node_to_node)?
            };
            terminal_node(&next, provides, seen)
        }
        other => Some(other.clone()),
    }
}

fn value_node_to_node(node: &ValueNode) -> Node {
    match node {
        ValueNode::Array(n) => n.clone().into(),
        ValueNode::Boolean(n) => (*n).into(),
        ValueNode::Bytes(n) => n.clone().into(),
        ValueNode::Constant(n) => n.clone().into(),
        ValueNode::Enum(n) => n.clone().into(),
        ValueNode::Injected(n) => n.clone().into(),
        ValueNode::Map(n) => n.clone().into(),
        ValueNode::None(n) => (*n).into(),
        ValueNode::Number(n) => (*n).into(),
        ValueNode::PublicKey(n) => n.clone().into(),
        ValueNode::Set(n) => n.clone().into(),
        ValueNode::Some(n) => n.clone().into(),
        ValueNode::String(n) => n.clone().into(),
        ValueNode::Struct(n) => n.clone().into(),
        ValueNode::Tuple(n) => n.clone().into(),
    }
}

fn number_from_node(node: &NumberValueNode) -> Value {
    let n = match node.number {
        Number::UnsignedInteger(u) => NumberValue::Unsigned(u as u128),
        Number::SignedInteger(i) => NumberValue::Signed(i as i128),
        Number::Float(f) => NumberValue::Float(f),
    };
    Value::Number(n)
}

fn number_node_u32(node: &NumberValueNode) -> Option<u32> {
    value_u32(&number_from_node(node))
}

fn find_account(parsed: &ParsedInstruction, name: &str) -> Option<Pubkey> {
    parsed
        .accounts
        .iter()
        .find(|a| a.name == name)
        .map(|a| a.meta.pubkey)
}

fn named_address(pk: Pubkey, names: &HashMap<Pubkey, String>) -> String {
    names.get(&pk).cloned().unwrap_or_else(|| pk.to_string())
}

fn struct_field<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    match value {
        Value::Struct(map) => map.get(name),
        _ => None,
    }
}

/// An argument, or a field inside one by its dotted path (`args.decimals`).
fn argument_at<'a>(data: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = data;
    for part in path.split('.') {
        current = struct_field(unwrap_some(current), part)?;
    }
    Some(current)
}

fn value_at<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for part in path.split('.') {
        current = struct_field(current, part)?;
    }
    Some(current)
}

fn member_hidden(
    display: Option<(Option<&str>, Option<DisplaySkip>)>,
    name: &str,
    consumed: &HashSet<String>,
) -> bool {
    match display.and_then(|(_, skip)| skip) {
        Some(DisplaySkip::Always) => true,
        Some(DisplaySkip::WhenInjected) => consumed.contains(name),
        Some(DisplaySkip::Never) | None => false,
    }
}

fn skip_always(skip: Option<DisplaySkip>) -> bool {
    matches!(skip, Some(DisplaySkip::Always))
}

fn member_label(label: Option<&str>, name: &str) -> String {
    match label {
        Some(label) if !label.is_empty() => label.to_string(),
        _ => title_case(name),
    }
}

fn default_text(value: &Value) -> String {
    match value {
        Value::Number(n) => match n {
            NumberValue::Unsigned(n) => n.to_string(),
            NumberValue::Signed(n) => n.to_string(),
            NumberValue::Float(n) => format_float(*n),
        },
        Value::Bool(b) => b.to_string(),
        Value::String(s) => s.clone(),
        Value::Bytes(b) => hex_bytes(b),
        Value::PublicKey(bytes) => Pubkey::new_from_array(*bytes).to_string(),
        Value::Array(items) | Value::Set(items) | Value::Tuple(items) => items
            .iter()
            .map(default_text)
            .collect::<Vec<_>>()
            .join(", "),
        Value::Map(entries) => entries
            .iter()
            .map(|(k, v)| format!("{}: {}", default_text(k), default_text(v)))
            .collect::<Vec<_>>()
            .join(", "),
        Value::Struct(map) => map
            .iter()
            .map(|(k, v)| format!("{}: {}", title_case(k), default_text(v)))
            .collect::<Vec<_>>()
            .join(", "),
        Value::Enum { variant, .. } => variant.clone(),
        Value::Option(None) => "None".to_string(),
        Value::Option(Some(inner)) => default_text(inner),
    }
}

fn raw_integer(value: &Value) -> Option<String> {
    match value {
        Value::Number(NumberValue::Unsigned(n)) => Some(n.to_string()),
        Value::Number(NumberValue::Signed(n)) => Some(n.to_string()),
        _ => None,
    }
}

fn value_u32(value: &Value) -> Option<u32> {
    let n = match value {
        Value::Number(NumberValue::Unsigned(n)) => i128::try_from(*n).ok()?,
        Value::Number(NumberValue::Signed(n)) => *n,
        Value::Number(NumberValue::Float(n)) if n.is_finite() && n.fract() == 0.0 => *n as i128,
        _ => return None,
    };
    u32::try_from(n).ok()
}

fn value_pubkey(value: &Value) -> Option<Pubkey> {
    match value {
        Value::PublicKey(bytes) => Some(Pubkey::new_from_array(*bytes)),
        _ => None,
    }
}

fn scale_value(value: &Value, decimals: u32) -> Option<String> {
    if decimals > 80 {
        return None;
    }
    match value {
        Value::Number(NumberValue::Unsigned(n)) => {
            Some(scale_digits(&n.to_string(), decimals, false))
        }
        Value::Number(NumberValue::Signed(n)) => Some(scale_digits(
            &n.unsigned_abs().to_string(),
            decimals,
            *n < 0,
        )),
        Value::Number(NumberValue::Float(n))
            if n.is_finite()
                && n.fract() == 0.0
                && *n >= i128::MIN as f64
                && *n <= i128::MAX as f64 =>
        {
            let n = *n as i128;
            Some(scale_digits(&n.unsigned_abs().to_string(), decimals, n < 0))
        }
        _ => None,
    }
}

fn scale_digits(digits: &str, decimals: u32, negative: bool) -> String {
    let body = if decimals == 0 {
        digits.to_string()
    } else {
        let width = decimals as usize + 1;
        let padded = if digits.len() >= width {
            digits.to_string()
        } else {
            format!("{digits:0>width$}")
        };
        let split = padded.len() - decimals as usize;
        let int_part = if split == 0 { "0" } else { &padded[..split] };
        let frac = padded[split..].trim_end_matches('0');
        if frac.is_empty() {
            int_part.to_string()
        } else {
            format!("{int_part}.{frac}")
        }
    };
    if negative && body != "0" {
        format!("-{body}")
    } else {
        body
    }
}

fn format_datetime(value: &Value, ticks_per_second: Option<u64>) -> Option<String> {
    let ticks = integer_i128(value)?;
    let tps = ticks_per_second.unwrap_or(1);
    if tps == 0 {
        return None;
    }
    let tps = tps as i128;
    let seconds = ticks.div_euclid(tps);
    let millis = ((ticks.rem_euclid(tps) * 1000) / tps) as u32;
    let seconds = i64::try_from(seconds).ok()?;
    format_iso(seconds, millis)
}

fn format_duration(value: &Value, ticks_per_second: Option<u64>) -> Option<String> {
    let ticks = integer_i128(value)?;
    let tps = ticks_per_second.unwrap_or(1);
    if tps == 0 {
        return None;
    }
    let seconds = ticks.div_euclid(tps as i128);
    if seconds < 0 {
        return None;
    }
    let seconds = seconds as u128;
    let hh = seconds / 3600;
    let mm = (seconds % 3600) / 60;
    let ss = seconds % 60;
    Some(format!("{hh:02}:{mm:02}:{ss:02}"))
}

fn integer_i128(value: &Value) -> Option<i128> {
    match value {
        Value::Number(NumberValue::Unsigned(n)) => i128::try_from(*n).ok(),
        Value::Number(NumberValue::Signed(n)) => Some(*n),
        Value::Number(NumberValue::Float(n))
            if n.is_finite()
                && n.fract() == 0.0
                && *n >= i128::MIN as f64
                && *n <= i128::MAX as f64 =>
        {
            Some(*n as i128)
        }
        _ => None,
    }
}

fn format_iso(seconds: i64, millis: u32) -> Option<String> {
    let days = seconds.div_euclid(86_400);
    let tod = seconds.rem_euclid(86_400) as u32;
    let (year, month, day) = civil_from_days(days);
    if !(0..10_000).contains(&year) {
        return None;
    }
    let hh = tod / 3600;
    let mm = (tod % 3600) / 60;
    let ss = tod % 60;
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{hh:02}:{mm:02}:{ss:02}.{millis:03}Z"
    ))
}

/// Howard Hinnant's `civil_from_days`. `z` is days since 1970-01-01.
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

fn slice_chars(s: &str, start: Option<u64>, end: Option<u64>) -> String {
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len() as u64;
    let start = start.unwrap_or(0).min(len) as usize;
    let end = end.unwrap_or(len).min(len) as usize;
    if end < start {
        String::new()
    } else {
        chars[start..end].iter().collect()
    }
}

fn format_float(n: f64) -> String {
    if n.is_finite() && n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

fn hex_bytes(bytes: &[u8]) -> String {
    const HEX: &[u8] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xf) as usize] as char);
    }
    out
}

fn title_case(name: &str) -> String {
    let mut out = String::new();
    let mut start = true;
    let chars: Vec<char> = name.chars().collect();
    for (i, ch) in chars.iter().copied().enumerate() {
        if ch == '_' || ch == '-' {
            if !out.is_empty() && !out.ends_with(' ') {
                out.push(' ');
            }
            start = true;
            continue;
        }
        let prev_lower = i > 0 && chars[i - 1].is_lowercase();
        if ch.is_uppercase() && prev_lower && !out.ends_with(' ') {
            out.push(' ');
        }
        if start {
            out.extend(ch.to_uppercase());
            start = false;
        } else {
            out.push(ch);
        }
    }
    out
}

fn pascal_case(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codama_dynamic_parsers::ParsedInstructionAccount;
    use codama_nodes::{
        AccountFieldValueNode, AccountLinkNode, AccountNode, AmountNumberDisplayNode,
        DateTimeNumberDisplayNode, DisplaySkip, DurationNumberDisplayNode,
        InjectableNumberValueNode, InstructionAccountDisplayNode, InstructionArgumentNode,
        InstructionDisplayNode, InstructionNode, NumberTypeNode, ProgramNode, ProvidedNode,
        StringDisplayNode, StringTypeNode, StructFieldDisplayNode, StructFieldTypeNode,
        StructTypeNode, U64, U8,
    };
    use codama_visitors_core::NodePath;

    fn pk(n: u8) -> Pubkey {
        Pubkey::new_from_array([n; 32])
    }

    fn parsed(
        root: &RootNode,
        instruction: InstructionNode,
        data: Value,
        accounts: Vec<ParsedInstructionAccount>,
    ) -> ParsedInstruction {
        ParsedInstruction {
            data,
            path: NodePath::from(vec![
                Node::Root(root.clone()),
                Node::Program(root.program.clone()),
                Node::Instruction(instruction),
            ]),
            accounts,
            remaining_accounts: vec![],
        }
    }

    #[test]
    fn epoch_formats_as_iso_utc() {
        assert_eq!(
            format_iso(0, 0).as_deref(),
            Some("1970-01-01T00:00:00.000Z")
        );
        assert_eq!(
            format_iso(1_700_000_000, 0).as_deref(),
            Some("2023-11-14T22:13:20.000Z")
        );
    }

    #[test]
    fn date_duration_and_string_slice() {
        let program = ProgramNode::new("prog", pk(1).to_string());
        let root = RootNode::new(program);
        let mut when = NumberTypeNode::le(U64);
        when.display = Box::new(Some(NumberDisplayNode::DateTime(
            DateTimeNumberDisplayNode {
                ticks_per_second: None,
            },
        )));
        let mut span = NumberTypeNode::le(U64);
        span.display = Box::new(Some(NumberDisplayNode::Duration(
            DurationNumberDisplayNode {
                ticks_per_second: Some(1),
            },
        )));
        let mut label = StringTypeNode::utf8();
        label.display = Some(StringDisplayNode {
            slice_start: Some(1),
            slice_end: Some(4),
        });
        let instruction = InstructionNode {
            name: "mark".into(),
            arguments: vec![
                InstructionArgumentNode::new("when", when),
                InstructionArgumentNode::new("span", span),
                InstructionArgumentNode::new("label", label),
            ],
            ..Default::default()
        };
        let parsed = parsed(
            &root,
            instruction,
            Value::struct_from([
                ("when".into(), 0u64.into()),
                ("span".into(), 3661u64.into()),
                ("label".into(), Value::String("hello".into())),
            ]),
            vec![],
        );
        let display =
            get_instruction_display(&root, &parsed, &HashMap::new(), &HashMap::new()).unwrap();
        assert_eq!(display.intent, "Mark");
        assert_eq!(
            display
                .fields
                .iter()
                .map(|f| f.value.as_str())
                .collect::<Vec<_>>(),
            vec!["1970-01-01T00:00:00.000Z", "01:01:01", "ell"]
        );
    }

    #[test]
    fn account_field_scales_amount_and_lists_required_account() {
        let mut program = ProgramNode::new("prog", pk(1).to_string());
        program.accounts = vec![AccountNode::new(
            "mint",
            StructTypeNode::new(vec![StructFieldTypeNode::new(
                "decimals",
                NumberTypeNode::le(U8),
            )]),
        )];
        let root = RootNode::new(program);
        let mut amount_ty = NumberTypeNode::le(U64);
        amount_ty.display = Box::new(Some(NumberDisplayNode::Amount(AmountNumberDisplayNode {
            decimals: Box::new(Some(InjectableNumberValueNode::Injected(
                codama_nodes::InjectedValueNode::new("decimals"),
            ))),
            unit: Box::new(None),
        })));
        let mut mint = InstructionAccountNode::new("mint", false, false);
        mint.account_link = Some(AccountLinkNode {
            name: "mint".into(),
            program: None,
        });
        mint.display = Some(InstructionAccountDisplayNode {
            label: Some("Mint".into()),
            skip: Some(DisplaySkip::WhenInjected),
        });
        let instruction = InstructionNode {
            name: "transfer".into(),
            display: Some(InstructionDisplayNode {
                intent: Some("Transfer tokens".into()),
                interpolated_intent: Some("Transfer ${data.amount}".into()),
            }),
            accounts: vec![mint],
            arguments: vec![InstructionArgumentNode::new("amount", amount_ty)],
            provides: vec![ProvidedNode {
                name: "decimals".into(),
                node: Box::new(
                    AccountFieldValueNode {
                        account: "mint".into(),
                        path: Some("decimals".into()),
                    }
                    .into(),
                ),
            }],
            ..Default::default()
        };
        let mint_pk = pk(4);
        let parsed = parsed(
            &root,
            instruction,
            Value::struct_from([("amount".into(), 1_000_000_000u64.into())]),
            vec![ParsedInstructionAccount {
                name: "mint".into(),
                meta: AccountMeta {
                    pubkey: mint_pk,
                    is_signer: false,
                    is_writable: false,
                },
            }],
        );
        assert_eq!(required_accounts_for_display(&root, &parsed), vec![mint_pk]);

        let missing =
            get_instruction_display(&root, &parsed, &HashMap::new(), &HashMap::new()).unwrap();
        assert!(missing.interpolated_intent.is_none());
        assert!(missing.spans.is_none());
        assert_eq!(missing.fields[0].value, "1000000000 (raw)");
        assert_eq!(missing.fields[1].label, "Mint");

        let mut accounts = HashMap::new();
        accounts.insert(mint_pk, vec![9u8]);
        let mut names = HashMap::new();
        names.insert(mint_pk, "My Mint".into());
        let shown = get_instruction_display(&root, &parsed, &accounts, &names).unwrap();
        assert_eq!(shown.interpolated_intent.as_deref(), Some("Transfer 1"));
        let spans = shown.spans.clone().unwrap();
        assert_eq!(join_spans(&spans), "Transfer 1");
        assert!(!shown
            .interpolated_intent
            .as_deref()
            .unwrap()
            .contains("My Mint"));
        match &spans[1] {
            IntentSpan::Value(value) => {
                assert_eq!(value.placeholder, "data.amount");
                assert_eq!(value.primary, "1");
                match &value.body {
                    IntentBody::Amount {
                        raw,
                        decimals,
                        scale_from,
                        ..
                    } => {
                        assert_eq!(raw, "1000000000");
                        assert_eq!(*decimals, Some(9));
                        assert_eq!(*scale_from, ScaleSource::Account(mint_pk));
                    }
                    other => panic!("expected an amount span, got {other:?}"),
                }
            }
            other => panic!("expected a value span, got {other:?}"),
        }
        assert_eq!(shown.fields.len(), 1);
        assert_eq!(shown.fields[0].value, "1");
    }

    #[test]
    fn skip_always_hides_field_but_not_placeholder() {
        let program = ProgramNode::new("prog", pk(1).to_string());
        let root = RootNode::new(program);
        let mut hidden = InstructionArgumentNode::new("tag", NumberTypeNode::le(U8));
        hidden.display = Some(StructFieldDisplayNode {
            skip: Some(DisplaySkip::Always),
            ..Default::default()
        });
        let instruction = InstructionNode {
            name: "go".into(),
            display: Some(InstructionDisplayNode {
                intent: Some("Go".into()),
                interpolated_intent: Some("Tag ${data.tag}".into()),
            }),
            arguments: vec![hidden],
            ..Default::default()
        };
        let parsed = parsed(
            &root,
            instruction,
            Value::struct_from([("tag".into(), 3u8.into())]),
            vec![],
        );
        let display =
            get_instruction_display(&root, &parsed, &HashMap::new(), &HashMap::new()).unwrap();
        assert_eq!(display.interpolated_intent.as_deref(), Some("Tag 3"));
        assert_eq!(join_spans(display.spans.as_ref().unwrap()), "Tag 3");
        assert!(display.fields.is_empty());
    }

    #[test]
    fn account_placeholder_keeps_the_address_and_ignores_the_dictionary_name() {
        let program = ProgramNode::new("prog", pk(1).to_string());
        let root = RootNode::new(program);
        let dest = pk(8);
        let instruction = InstructionNode {
            name: "send".into(),
            display: Some(InstructionDisplayNode {
                intent: Some("Send".into()),
                interpolated_intent: Some("To ${accounts.destination}".into()),
            }),
            accounts: vec![InstructionAccountNode::new("destination", true, false)],
            ..Default::default()
        };
        let parsed = parsed(
            &root,
            instruction,
            Value::Struct(Default::default()),
            vec![ParsedInstructionAccount {
                name: "destination".into(),
                meta: AccountMeta {
                    pubkey: dest,
                    is_signer: false,
                    is_writable: true,
                },
            }],
        );
        let mut names = HashMap::new();
        names.insert(dest, "Treasury".into());
        let display = get_instruction_display(&root, &parsed, &HashMap::new(), &names).unwrap();
        let sentence = display.interpolated_intent.clone().unwrap();
        assert_eq!(sentence, format!("To {dest}"));
        assert!(!sentence.contains("Treasury"));
        assert_eq!(join_spans(display.spans.as_ref().unwrap()), sentence);
        match &display.spans.unwrap()[1] {
            IntentSpan::Value(value) => match &value.body {
                IntentBody::Address(pk) => assert_eq!(*pk, dest),
                other => panic!("expected an address, got {other:?}"),
            },
            other => panic!("expected a value span, got {other:?}"),
        }
    }

    /// `args { amount, decimals }` flattened, with `amount` scaled by
    /// `args.decimals` and named in the sentence by its dotted path.
    fn nested_args_instruction(template: &str) -> (RootNode, InstructionNode) {
        let root = RootNode::new(ProgramNode::new("prog", pk(1).to_string()));
        let mut amount_ty = NumberTypeNode::le(U64);
        amount_ty.display = Box::new(Some(NumberDisplayNode::Amount(AmountNumberDisplayNode {
            decimals: Box::new(Some(InjectableNumberValueNode::Injected(
                codama_nodes::InjectedValueNode::new("decimals"),
            ))),
            unit: Box::new(None),
        })));
        let mut decimals = StructFieldTypeNode::new("decimals", NumberTypeNode::le(U8));
        decimals.display = Some(StructFieldDisplayNode {
            skip: Some(DisplaySkip::WhenInjected),
            ..Default::default()
        });
        let mut args = InstructionArgumentNode::new(
            "args",
            StructTypeNode::new(vec![
                StructFieldTypeNode::new("amount", amount_ty),
                decimals,
            ]),
        );
        args.display = Some(StructFieldDisplayNode {
            flatten: Some(true),
            ..Default::default()
        });
        // Built from JSON: `ArgumentValueNode::new` would camel-case the path.
        let provided: ProvidedNode = serde_json::from_value(serde_json::json!({
            "kind": "providedNode",
            "name": "decimals",
            "node": { "kind": "argumentValueNode", "name": "args.decimals" }
        }))
        .unwrap();
        let instruction = InstructionNode {
            name: "send".into(),
            display: Some(InstructionDisplayNode {
                intent: Some("Send".into()),
                interpolated_intent: Some(template.into()),
            }),
            arguments: vec![args],
            provides: vec![provided],
            ..Default::default()
        };
        (root, instruction)
    }

    fn nested_args_data() -> Value {
        Value::struct_from([(
            "args".into(),
            Value::struct_from([
                ("amount".into(), 1_500_000u64.into()),
                ("decimals".into(), 6u8.into()),
            ]),
        )])
    }

    #[test]
    fn a_placeholder_reaches_into_an_argument_by_its_dotted_path() {
        let (root, instruction) = nested_args_instruction("Send ${data.args.amount}");
        let parsed = parsed(&root, instruction, nested_args_data(), vec![]);
        let shown =
            get_instruction_display(&root, &parsed, &HashMap::new(), &HashMap::new()).unwrap();
        assert_eq!(shown.interpolated_intent.as_deref(), Some("Send 1.5"));
        match &shown.spans.as_ref().unwrap()[1] {
            IntentSpan::Value(value) => {
                assert_eq!(value.placeholder, "data.args.amount");
                assert!(matches!(
                    &value.body,
                    IntentBody::Amount { raw, decimals: Some(6), scale_from: ScaleSource::Instruction, .. }
                        if raw == "1500000"
                ));
            }
            other => panic!("expected a value span, got {other:?}"),
        }
        // The decimals were injected from `args.decimals`, so that field is
        // hidden and the amount line is scaled.
        assert_eq!(shown.fields.len(), 1);
        assert_eq!(shown.fields[0].label, "Amount");
        assert_eq!(shown.fields[0].value, "1.5");
    }

    #[test]
    fn a_dotted_path_that_does_not_resolve_leaves_no_sentence() {
        for template in [
            "Send ${data.args.missing}",
            "Send ${data.args.amount.deeper}",
            "Send ${data.nothing.amount}",
        ] {
            let (root, instruction) = nested_args_instruction(template);
            let parsed = parsed(&root, instruction, nested_args_data(), vec![]);
            let shown =
                get_instruction_display(&root, &parsed, &HashMap::new(), &HashMap::new()).unwrap();
            assert!(shown.interpolated_intent.is_none(), "{template}");
        }
    }
}
