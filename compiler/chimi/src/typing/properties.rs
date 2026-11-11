//! The derived equatable and ordered properties.
//!
//! Both walk a type's structure, the way the shareable derivation does, and
//! stop at different leaves: functions, interfaces, and capabilities are
//! neither; records, maps, `Json.Value`, and failure sets of several members are
//! equatable but not ordered. A failed derivation records the path to the first
//! component that lacks the property, so diagnostics can name it.

use super::{CanonicalType, FailureRow, FieldShape};
use crate::ids::{SymbolId, TypeId};
use crate::resolution::{ProjectSymbolKind, ResolvedProject};
use crate::standard::{StandardProperties, StandardSymbol};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
pub(crate) enum ValueProperty {
    Equatable,
    /// Implies `Equatable`.
    Ordered,
}

impl ValueProperty {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Equatable => "equatable",
            Self::Ordered => "ordered",
        }
    }
}

/// The component that lacks a property, and how to reach it from the checked type.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PropertyLack {
    /// Outermost step first, such as `["element of `List`", "field `handler` of `Route`"]`.
    steps: Vec<String>,
    /// What the component is, such as "a function".
    culprit: String,
}

impl PropertyLack {
    /// "field `handler` of `Route` is a function", innermost step first.
    pub(crate) fn describe(&self) -> String {
        let mut steps = self.steps.iter().rev();
        match steps.next() {
            None => format!("it is {}", self.culprit),
            Some(innermost) => {
                let mut text = innermost.clone();
                for step in steps {
                    text.push_str(", within ");
                    text.push_str(step);
                }
                format!("{text} is {}", self.culprit)
            }
        }
    }
}

/// The outcome of deriving one property for one type.
#[derive(Debug, Default)]
pub(crate) struct Derivation {
    /// Set when the type certainly lacks the property.
    pub(crate) lack: Option<PropertyLack>,
    /// Generic parameters of the enclosing declaration the property depends on.
    /// Each must carry the property as an inferred requirement.
    pub(crate) generics: Vec<SymbolId>,
    /// Set when an unsolved inference variable leaves the answer open.
    pub(crate) unknown: bool,
}

/// The parts of the type arena a derivation reads; both the checker and the
/// finished project lend theirs.
pub(crate) struct TypeView<'a> {
    pub(crate) resolved: &'a ResolvedProject,
    pub(crate) types: &'a [CanonicalType],
    pub(crate) fields: &'a HashMap<SymbolId, Vec<FieldShape>>,
    pub(crate) failure_rows: &'a [FailureRow],
    pub(crate) inference_bindings: &'a HashMap<u32, TypeId>,
}

/// Generic arguments in scope: a named type's parameters bound to the argument
/// types written in the frame that named it.
struct Frame {
    bindings: HashMap<SymbolId, (TypeId, usize)>,
}

/// Nesting beyond this depth only arises from polymorphic recursion, which the
/// greatest-fixed-point rule accepts.
const DEPTH_LIMIT: usize = 64;

impl TypeView<'_> {
    pub(crate) fn derive(&self, type_id: TypeId, property: ValueProperty) -> Derivation {
        let mut walk = Walk {
            view: self,
            property,
            frames: vec![Frame {
                bindings: HashMap::new(),
            }],
            visiting: HashSet::new(),
            derivation: Derivation::default(),
        };
        let lack = walk.visit(type_id, 0, 0);
        let mut derivation = walk.derivation;
        derivation.lack = lack;
        derivation.generics.sort_unstable();
        derivation.generics.dedup();
        derivation
    }

    fn resolve(&self, mut id: TypeId) -> TypeId {
        while let CanonicalType::InferenceVariable(variable) = self.types[id.raw() as usize] {
            match self.inference_bindings.get(&variable) {
                Some(bound) if *bound != id => id = *bound,
                _ => break,
            }
        }
        id
    }

    fn name(&self, symbol: SymbolId) -> &str {
        &self.resolved.symbol(symbol).name
    }

    fn type_generic_parameters(&self, owner: SymbolId) -> Vec<SymbolId> {
        let owner = self.resolved.symbol(owner);
        let (Some(module_id), Some(semantic_id)) = (owner.module_id, owner.semantic_id) else {
            return Vec::new();
        };
        let Some(declaration) = self.resolved.semantic.modules[module_id.raw() as usize]
            .declarations
            .iter()
            .find(|declaration| declaration.semantic_id == semantic_id)
        else {
            return Vec::new();
        };
        declaration
            .generic_parameters
            .iter()
            .filter_map(|parameter| {
                self.resolved.symbols.iter().find(|symbol| {
                    symbol.module_id == Some(module_id)
                        && symbol.semantic_id == Some(semantic_id)
                        && symbol.kind == ProjectSymbolKind::GenericParameter
                        && symbol.name == parameter.text
                })
            })
            .map(|symbol| symbol.id)
            .collect()
    }

    /// A source enum's variants in declaration order, with their names.
    fn enum_variants(&self, owner: SymbolId) -> Vec<(SymbolId, String)> {
        let owner = self.resolved.symbol(owner);
        let (Some(module_id), Some(semantic_id)) = (owner.module_id, owner.semantic_id) else {
            return Vec::new();
        };
        let Some(declaration) = self.resolved.semantic.modules[module_id.raw() as usize]
            .declarations
            .iter()
            .find(|declaration| declaration.semantic_id == semantic_id)
        else {
            return Vec::new();
        };
        declaration
            .variants
            .iter()
            .filter_map(|variant| {
                self.resolved
                    .symbols
                    .iter()
                    .find(|symbol| {
                        symbol.module_id == Some(module_id)
                            && symbol.semantic_id == Some(variant.semantic_id)
                            && symbol.kind == ProjectSymbolKind::EnumConstructor
                    })
                    .map(|symbol| (symbol.id, variant.name.text.clone()))
            })
            .collect()
    }
}

struct Walk<'view, 'a> {
    view: &'view TypeView<'a>,
    property: ValueProperty,
    frames: Vec<Frame>,
    visiting: HashSet<(SymbolId, Vec<(TypeId, usize)>)>,
    derivation: Derivation,
}

impl Walk<'_, '_> {
    fn lack(culprit: impl Into<String>) -> Option<PropertyLack> {
        Some(PropertyLack {
            steps: Vec::new(),
            culprit: culprit.into(),
        })
    }

    fn within(step: impl Into<String>, lack: Option<PropertyLack>) -> Option<PropertyLack> {
        lack.map(|mut lack| {
            lack.steps.insert(0, step.into());
            lack
        })
    }

    fn ordered(&self) -> bool {
        self.property == ValueProperty::Ordered
    }

    /// Returns the first component that lacks the property, if any; records
    /// generic dependencies and open variables in `self.derivation` as it goes.
    fn visit(&mut self, id: TypeId, frame: usize, depth: usize) -> Option<PropertyLack> {
        if depth > DEPTH_LIMIT {
            return None;
        }
        let id = self.view.resolve(id);
        match self.view.types[id.raw() as usize].clone() {
            CanonicalType::Generic(symbol) => match self.frames[frame].bindings.get(&symbol) {
                Some(&(bound, bound_frame)) => self.visit(bound, bound_frame, depth + 1),
                None => {
                    self.derivation.generics.push(symbol);
                    None
                }
            },
            CanonicalType::InferenceVariable(_) => {
                self.derivation.unknown = true;
                None
            }
            CanonicalType::Poison => None,
            CanonicalType::Function(_) | CanonicalType::PolymorphicFunction { .. } => {
                Self::lack("a function")
            }
            CanonicalType::Tuple(elements) => {
                elements.iter().enumerate().find_map(|(index, element)| {
                    let lack = self.visit(*element, frame, depth + 1);
                    Self::within(format!("element {} of a tuple", index + 1), lack)
                })
            }
            CanonicalType::Record(fields) => {
                if self.ordered() {
                    return Self::lack("a record, which has no declaration order");
                }
                fields.iter().find_map(|field| {
                    let lack = self.visit(field.type_id, frame, depth + 1);
                    Self::within(format!("field `{}` of a record", field.name), lack)
                })
            }
            CanonicalType::FailureSetValue(row) => {
                let row = self.view.failure_rows[row.raw() as usize].clone();
                if self.ordered() && row.variants.len() + row.parameters.len() > 1 {
                    return Self::lack("a failure set of several members, which has no order");
                }
                row.variants.iter().find_map(|variant| {
                    let fields = self.view.fields.get(variant).cloned().unwrap_or_default();
                    let name = self.view.name(*variant).to_owned();
                    fields.iter().find_map(|field| {
                        let lack = self.visit(field.type_id, frame, depth + 1);
                        Self::within(format!("field `{}` of `{name}`", field.name), lack)
                    })
                })
            }
            CanonicalType::Named { symbol, arguments } => {
                self.visit_named(symbol, &arguments, frame, depth)
            }
        }
    }

    fn visit_named(
        &mut self,
        symbol: SymbolId,
        arguments: &[TypeId],
        frame: usize,
        depth: usize,
    ) -> Option<PropertyLack> {
        let name = self.view.name(symbol).to_owned();
        let resolved_symbol = self.view.resolved.symbol(symbol);
        let structural_arguments = |walk: &mut Self, step: &str| {
            arguments.iter().find_map(|argument| {
                let lack = walk.visit(*argument, frame, depth + 1);
                Self::within(format!("{step} `{name}`"), lack)
            })
        };
        if let Some(StandardSymbol::Type(kind)) = self.view.resolved.standard_symbol(symbol) {
            return match kind.properties() {
                StandardProperties::Structural if name == "Option" => {
                    structural_arguments(self, "payload of")
                }
                StandardProperties::Structural => structural_arguments(self, "type argument of"),
                StandardProperties::Unordered if self.ordered() => {
                    Self::lack(format!("`{name}`, which has no order"))
                }
                StandardProperties::Unordered => {
                    let mut lack = None;
                    for (argument, step) in arguments.iter().zip(["key of", "value of"]) {
                        lack = self.visit(*argument, frame, depth + 1);
                        if lack.is_some() {
                            lack = Self::within(format!("{step} `{name}`"), lack);
                            break;
                        }
                    }
                    lack
                }
                StandardProperties::EquatableOnly(reason) if self.ordered() => Self::lack(reason),
                StandardProperties::EquatableOnly(_) => None,
                StandardProperties::Opaque(culprit) => Self::lack(culprit),
            };
        }
        if resolved_symbol.kind == ProjectSymbolKind::BuiltinType
            && resolved_symbol.module_id.is_none()
        {
            return match name.as_str() {
                "List" => structural_arguments(self, "element of"),
                "Option" => structural_arguments(self, "payload of"),
                _ => None,
            };
        }
        match resolved_symbol.kind {
            ProjectSymbolKind::Interface => {
                return Self::lack(format!("an interface value of `{name}`"));
            }
            ProjectSymbolKind::Struct | ProjectSymbolKind::Enum => {}
            _ => return None,
        }
        let bound = arguments
            .iter()
            .map(|argument| (*argument, frame))
            .collect::<Vec<_>>();
        if !self.visiting.insert((symbol, bound.clone())) {
            // Greatest fixed point: a recursive occurrence has the property
            // unless some non-recursive component lacks it.
            return None;
        }
        let parameters = self.view.type_generic_parameters(symbol);
        self.frames.push(Frame {
            bindings: parameters.into_iter().zip(bound.iter().copied()).collect(),
        });
        let inner = self.frames.len() - 1;
        let components = if resolved_symbol.kind == ProjectSymbolKind::Enum {
            self.view
                .enum_variants(symbol)
                .into_iter()
                .flat_map(|(variant, variant_name)| {
                    let name = name.clone();
                    self.view
                        .fields
                        .get(&variant)
                        .into_iter()
                        .flatten()
                        .map(move |field| {
                            (
                                format!("field `{}` of `{name}.{variant_name}`", field.name),
                                field.type_id,
                            )
                        })
                })
                .collect::<Vec<_>>()
        } else {
            self.view
                .fields
                .get(&symbol)
                .into_iter()
                .flatten()
                .map(|field| (format!("field `{}` of `{name}`", field.name), field.type_id))
                .collect()
        };
        let lack = components.into_iter().find_map(|(step, type_id)| {
            let lack = self.visit(type_id, inner, depth + 1);
            Self::within(step, lack)
        });
        self.visiting.remove(&(symbol, bound));
        lack
    }
}
