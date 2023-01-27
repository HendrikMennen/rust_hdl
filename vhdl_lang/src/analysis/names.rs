// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this file,
// You can obtain one at http://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2022, Olof Kraigher olof.kraigher@gmail.com

use super::analyze::*;
use super::named_entity::*;
use super::overloaded::Disambiguated;
use super::overloaded::DisambiguatedType;
use super::region::*;
use crate::ast::*;
use crate::data::*;

macro_rules! try_unknown {
    ($expr:expr) => {
        if let Some(value) = $expr? {
            value
        } else {
            // Unknown
            return Ok(None);
        }
    };
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ObjectBase<'a> {
    Object(ObjectEnt<'a>),
    ObjectAlias(ObjectEnt<'a>, EntRef<'a>),
    DeferredConstant(EntRef<'a>),
    ExternalName(ExternalObjectClass),
}

impl<'a> ObjectBase<'a> {
    pub fn mode(&self) -> Option<Mode> {
        match self {
            ObjectBase::Object(object) => object.mode(),
            ObjectBase::ObjectAlias(object, _) => object.mode(),
            ObjectBase::DeferredConstant(..) => None,
            ObjectBase::ExternalName(_) => None,
        }
    }

    pub fn class(&self) -> ObjectClass {
        match self {
            ObjectBase::Object(object) => object.class(),
            ObjectBase::ObjectAlias(object, _) => object.class(),
            ObjectBase::DeferredConstant(..) => ObjectClass::Constant,
            ObjectBase::ExternalName(class) => (*class).into(),
        }
    }

    // Use whenever the class and mode is relevant to the error
    pub fn describe_class(&self) -> String {
        if let Some(mode) = self.mode() {
            if self.class() == ObjectClass::Constant {
                format!("interface {}", self.describe())
            } else {
                format!("interface {} of mode {}", self.describe(), mode)
            }
        } else {
            self.describe()
        }
    }

    pub fn describe(&self) -> String {
        match self {
            ObjectBase::DeferredConstant(ent) => {
                format!("deferred constant '{}'", ent.designator())
            }
            ObjectBase::ExternalName(..) => "external name".to_owned(),
            ObjectBase::Object(obj) => obj.describe_name(),
            ObjectBase::ObjectAlias(_, alias) => {
                format!("alias '{}' of {}", alias.designator(), self.class())
            }
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct ObjectName<'a> {
    pub base: ObjectBase<'a>,
    pub type_mark: Option<TypeEnt<'a>>,
}

impl<'a> ObjectName<'a> {
    pub fn type_mark(&self) -> TypeEnt<'a> {
        if let Some(type_mark) = self.type_mark {
            type_mark
        } else if let ObjectBase::Object(obj) = self.base {
            obj.type_mark()
        } else {
            unreachable!("No type mark implies object base")
        }
    }

    fn with_suffix(self, type_mark: TypeEnt<'a>) -> Self {
        ObjectName {
            base: self.base,
            type_mark: Some(type_mark),
        }
    }

    /// Use in error messages that focus on the type rather than class/mode
    pub fn describe_type(&self) -> String {
        if let Some(type_mark) = self.type_mark {
            type_mark.describe()
        } else {
            format!(
                "{} of {}",
                self.base.describe(),
                self.type_mark().describe()
            )
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ResolvedName<'a> {
    Library(Symbol),
    Design(DesignEnt<'a>),
    Type(TypeEnt<'a>),
    Overloaded(WithPos<Designator>, OverloadedName<'a>),
    ObjectName(ObjectName<'a>),
    /// The result of a function call and any subsequent selections thereof
    Expression(DisambiguatedType<'a>),
    // Something that cannot be further selected
    Final(EntRef<'a>),
}

impl<'a> ResolvedName<'a> {
    /// The name was selected out of a design unit
    fn from_design_not_overloaded(ent: &'a AnyEnt) -> Result<Self, String> {
        let name = match ent.kind() {
            AnyEntKind::Object(_) => ResolvedName::ObjectName(ObjectName {
                base: ObjectBase::Object(ObjectEnt::new(ent)),
                type_mark: None,
            }),
            AnyEntKind::ObjectAlias {
                base_object,
                type_mark,
            } => ResolvedName::ObjectName(ObjectName {
                base: ObjectBase::ObjectAlias(*base_object, ent),
                type_mark: Some(type_mark.to_owned()),
            }),
            AnyEntKind::ExternalAlias { class, type_mark } => {
                ResolvedName::ObjectName(ObjectName {
                    base: ObjectBase::ExternalName(*class),
                    type_mark: Some(*type_mark),
                })
            }
            AnyEntKind::DeferredConstant(subtype) => ResolvedName::ObjectName(ObjectName {
                base: ObjectBase::DeferredConstant(ent),
                type_mark: Some(subtype.type_mark()),
            }),
            AnyEntKind::Type(_) => ResolvedName::Type(TypeEnt::from_any(ent).unwrap()),
            AnyEntKind::Overloaded(_) => {
                return Err(
                    "Internal error. Unreachable as overloaded is handled outside".to_owned(),
                )
            }
            AnyEntKind::File(_)
            | AnyEntKind::InterfaceFile(_)
            | AnyEntKind::Component(_)
            | AnyEntKind::PhysicalLiteral(_) => ResolvedName::Final(ent),
            AnyEntKind::Design(_)
            | AnyEntKind::Library
            | AnyEntKind::Attribute(_)
            | AnyEntKind::ElementDeclaration(_)
            | AnyEntKind::Label
            | AnyEntKind::LoopParameter => {
                return Err(format!(
                    "{} cannot be selected from design unit",
                    ent.kind().describe()
                ))
            }
        };

        Ok(name)
    }

    /// The name was looked up from the current scope
    fn from_scope_not_overloaded(ent: &'a AnyEnt) -> Result<Self, String> {
        let name = match ent.kind() {
            AnyEntKind::Object(_) => ResolvedName::ObjectName(ObjectName {
                base: ObjectBase::Object(ObjectEnt::new(ent)),
                type_mark: None,
            }),
            AnyEntKind::ObjectAlias {
                base_object,
                type_mark,
            } => ResolvedName::ObjectName(ObjectName {
                base: ObjectBase::ObjectAlias(*base_object, ent),
                type_mark: Some(type_mark.to_owned()),
            }),
            AnyEntKind::ExternalAlias { class, type_mark } => {
                ResolvedName::ObjectName(ObjectName {
                    base: ObjectBase::ExternalName(*class),
                    type_mark: Some(*type_mark),
                })
            }
            AnyEntKind::DeferredConstant(subtype) => ResolvedName::ObjectName(ObjectName {
                base: ObjectBase::DeferredConstant(ent),
                type_mark: Some(subtype.type_mark()),
            }),
            AnyEntKind::Type(_) => ResolvedName::Type(TypeEnt::from_any(ent).unwrap()),
            AnyEntKind::Design(_) => ResolvedName::Design(DesignEnt::from_any(ent).unwrap()),
            AnyEntKind::Library => {
                ResolvedName::Library(ent.designator().as_identifier().cloned().unwrap())
            }
            AnyEntKind::Overloaded(_) => {
                return Err(
                    "Internal error. Unreachable as overloded is handled outside this function"
                        .to_string(),
                )
            }
            AnyEntKind::File(_)
            | AnyEntKind::InterfaceFile(_)
            | AnyEntKind::Component(_)
            | AnyEntKind::Label
            | AnyEntKind::LoopParameter
            | AnyEntKind::PhysicalLiteral(_) => ResolvedName::Final(ent),
            AnyEntKind::Attribute(_) | AnyEntKind::ElementDeclaration(_) => {
                return Err(format!(
                    "{} should never be looked up from the current scope",
                    ent.kind().describe()
                ))
            }
        };

        Ok(name)
    }

    /// A description that includes the type of the name
    /// This is used in contexts where the type is relevant to the error
    pub fn describe_type(&self) -> String {
        match self {
            ResolvedName::ObjectName(oname) => oname.describe_type(),
            ResolvedName::Expression(DisambiguatedType::Unambiguous(typ)) => {
                format!("Expression of {}", typ.describe())
            }
            _ => self.describe(),
        }
    }

    /// A description that does not include the name of the type
    /// This is used in contexts where the type is not relevant
    /// Such as when assigning to a constant
    pub fn describe(&self) -> String {
        match self {
            ResolvedName::Library(sym) => format!("library {sym}"),
            ResolvedName::Design(ent) => ent.describe(),
            ResolvedName::Type(ent) => ent.describe(),
            ResolvedName::Overloaded(des, name) => {
                if let Some(ent) = name.as_unique() {
                    ent.describe()
                } else {
                    format!("Overloaded name {des}")
                }
            }
            ResolvedName::ObjectName(oname) => oname.base.describe(),
            ResolvedName::Final(ent) => ent.describe(),
            ResolvedName::Expression(DisambiguatedType::Unambiguous(_)) => "Expression".to_owned(),
            ResolvedName::Expression(_) => "Ambiguous expression".to_owned(),
        }
    }
}
#[derive(Debug)]
pub struct AttributeSuffix<'a> {
    pub signature: &'a mut Option<WithPos<crate::ast::Signature>>,
    pub attr: &'a mut WithPos<AttributeDesignator>,
    pub expr: &'a mut Option<Box<WithPos<Expression>>>,
}

#[derive(Debug)]
enum Suffix<'a> {
    Selected(&'a mut WithPos<WithRef<Designator>>),
    All,
    Slice(&'a mut DiscreteRange),
    Attribute(AttributeSuffix<'a>),
    CallOrIndexed(&'a mut [AssociationElement]),
}

enum SplitName<'a> {
    Designator(&'a mut WithRef<Designator>),
    External(&'a mut ExternalName),
    Suffix(&'a mut WithPos<Name>, Suffix<'a>),
}

impl<'a> SplitName<'a> {
    fn from_name(name: &'a mut Name) -> SplitName<'a> {
        match name {
            Name::Designator(d) => SplitName::Designator(d),
            Name::External(e) => SplitName::External(e),
            Name::Selected(prefix, suffix) => {
                SplitName::Suffix(prefix.as_mut(), Suffix::Selected(suffix))
            }
            Name::SelectedAll(ref mut prefix) => SplitName::Suffix(prefix.as_mut(), Suffix::All),
            Name::Slice(ref mut prefix, range) => {
                SplitName::Suffix(prefix.as_mut(), Suffix::Slice(range))
            }
            Name::Attribute(ref mut attr) => SplitName::Suffix(
                &mut attr.name,
                Suffix::Attribute(AttributeSuffix {
                    signature: &mut attr.signature,
                    attr: &mut attr.attr,
                    expr: &mut attr.expr,
                }),
            ),
            Name::CallOrIndexed(ref mut fcall) => SplitName::Suffix(
                &mut fcall.name,
                Suffix::CallOrIndexed(&mut fcall.parameters),
            ),
        }
    }
}

enum TypeOrMethod<'a> {
    Type(TypeEnt<'a>),
    Method(WithPos<Designator>, OverloadedName<'a>),
}

fn could_be_indexed_name(assocs: &[AssociationElement]) -> bool {
    assocs
        .iter()
        .all(|assoc| assoc.formal.is_none() && !matches!(assoc.actual.item, ActualPart::Open))
}

impl<'a> AnalyzeContext<'a> {
    fn name_to_type(
        &self,
        pos: &SrcPos,
        name: ResolvedName<'a>,
    ) -> Result<Option<DisambiguatedType<'a>>, Diagnostic> {
        match name {
            ResolvedName::Library(_) | ResolvedName::Design(_) | ResolvedName::Type(_) => {
                Err(Diagnostic::error(
                    pos,
                    format!("{} cannot be used in an expression", name.describe_type()),
                ))
            }
            ResolvedName::Final(ent) => match ent.actual_kind() {
                AnyEntKind::LoopParameter => {
                    // TODO cannot handle yet
                    Ok(None)
                }
                AnyEntKind::PhysicalLiteral(typ) => Ok(Some(DisambiguatedType::Unambiguous(*typ))),
                AnyEntKind::File(subtype) => {
                    Ok(Some(DisambiguatedType::Unambiguous(subtype.type_mark())))
                }
                AnyEntKind::InterfaceFile(typ) => Ok(Some(DisambiguatedType::Unambiguous(*typ))),
                _ => Err(Diagnostic::error(
                    pos,
                    format!("{} cannot be used in an expression", name.describe_type()),
                )),
            },
            ResolvedName::Overloaded(des, overloaded) => {
                if let Some(disamb) = self.disambiguate_no_actuals(&des, None, &overloaded)? {
                    Ok(Some(disamb.into_type()))
                } else {
                    Ok(None)
                }
            }
            ResolvedName::ObjectName(oname) => {
                Ok(Some(DisambiguatedType::Unambiguous(oname.type_mark())))
            }
            ResolvedName::Expression(expr_type) => Ok(Some(expr_type)),
        }
    }

    fn name_to_unambiguous_type(
        &self,
        pos: &SrcPos,
        name: &ResolvedName<'a>,
        ttyp: TypeEnt<'a>,
        // Optional reference to set when disambiguating overloaded
        suffix_ref: Option<&mut Reference>,
    ) -> Result<Option<TypeEnt<'a>>, Diagnostic> {
        match name {
            ResolvedName::Library(_) | ResolvedName::Design(_) | ResolvedName::Type(_) => {
                Err(Diagnostic::error(
                    pos,
                    format!("{} cannot be used in an expression", name.describe_type()),
                ))
            }
            ResolvedName::Final(ent) => match ent.actual_kind() {
                AnyEntKind::LoopParameter => {
                    // TODO cannot handle yet
                    Ok(None)
                }
                AnyEntKind::PhysicalLiteral(typ) => Ok(Some(*typ)),
                AnyEntKind::File(subtype) => Ok(Some(subtype.type_mark())),
                AnyEntKind::InterfaceFile(typ) => Ok(Some(*typ)),
                _ => Err(Diagnostic::error(
                    pos,
                    format!("{} cannot be used in an expression", name.describe_type()),
                )),
            },
            ResolvedName::Overloaded(des, overloaded) => {
                if let Some(disamb) = self.disambiguate_no_actuals(des, Some(ttyp), overloaded)? {
                    match disamb {
                        Disambiguated::Unambiguous(ent) => {
                            if let Some(reference) = suffix_ref {
                                *reference = Some(ent.id());
                            }
                            Ok(Some(ent.return_type().unwrap()))
                        }
                        Disambiguated::Ambiguous(overloaded) => {
                            Err(Diagnostic::ambiguous_call(des, overloaded))
                        }
                    }
                } else {
                    Ok(None)
                }
            }
            ResolvedName::ObjectName(oname) => Ok(Some(oname.type_mark())),
            ResolvedName::Expression(DisambiguatedType::Unambiguous(typ)) => Ok(Some(*typ)),
            ResolvedName::Expression(DisambiguatedType::Ambiguous(_)) => {
                // @TODO show ambigous error
                Ok(None)
            }
        }
    }

    /// An array type may be sliced with a type name
    /// For the parser this looks like a call or indexed name
    /// Example:
    /// subtype sub_t is natural range 0 to 1;
    /// arr(sub_t) := (others => 0);
    fn assoc_as_discrete_range_type(
        &self,
        scope: &Scope<'a>,
        assocs: &mut [AssociationElement],
        diagnostics: &mut dyn DiagnosticHandler,
    ) -> AnalysisResult<Option<TypeEnt<'a>>> {
        if !could_be_indexed_name(assocs) {
            return Ok(None);
        }

        if let [ref mut assoc] = assocs {
            if let ActualPart::Expression(expr) = &mut assoc.actual.item {
                return self.expr_as_discrete_range_type(
                    scope,
                    &assoc.actual.pos,
                    expr,
                    diagnostics,
                );
            }
        }
        Ok(None)
    }

    pub fn expr_as_discrete_range_type(
        &self,
        scope: &Scope<'a>,
        expr_pos: &SrcPos,
        expr: &mut Expression,
        diagnostics: &mut dyn DiagnosticHandler,
    ) -> AnalysisResult<Option<TypeEnt<'a>>> {
        if let Expression::Name(name) = expr {
            if !name.is_selected_name() {
                // Early exit
                return Ok(None);
            }

            let resolved = self.name_resolve(scope, expr_pos, name, diagnostics)?;

            if let Some(ResolvedName::Type(typ)) = resolved {
                return if matches!(typ.base_type().kind(), Type::Enum { .. } | Type::Integer(_)) {
                    Ok(Some(typ))
                } else {
                    Err(Diagnostic::error(
                        expr_pos,
                        format!("{} cannot be used as a discrete range", typ.describe()),
                    )
                    .into())
                };
            }
        }

        Ok(None)
    }

    // Apply suffix when prefix is known to have a type
    // The prefix may be an object or a function return value
    fn resolve_typed_suffix(
        &self,
        scope: &Scope<'a>,
        prefix_pos: &SrcPos,
        name_pos: &SrcPos,
        prefix_typ: TypeEnt<'a>,
        suffix: &mut Suffix,
        diagnostics: &mut dyn DiagnosticHandler,
    ) -> AnalysisResult<Option<TypeOrMethod<'a>>> {
        match suffix {
            Suffix::Selected(suffix) => {
                Ok(Some(match prefix_typ.selected(prefix_pos, suffix)? {
                    TypedSelection::RecordElement(elem) => {
                        suffix.set_unique_reference(&elem);
                        TypeOrMethod::Type(elem.type_mark())
                    }
                    TypedSelection::ProtectedMethod(name) => TypeOrMethod::Method(
                        WithPos::new(suffix.item.item.clone(), suffix.pos.clone()),
                        name,
                    ),
                }))
            }
            Suffix::All => Ok(prefix_typ.accessed_type().map(TypeOrMethod::Type)),
            Suffix::Slice(drange) => Ok(if let Some(typ) = prefix_typ.sliced_as() {
                // @TODO check drange type
                self.analyze_discrete_range(scope, drange, diagnostics)?;
                Some(TypeOrMethod::Type(typ))
            } else {
                None
            }),
            // @TODO attribute not handled
            Suffix::Attribute(_) => Ok(None),
            // @TODO Prefix must non-overloaded
            Suffix::CallOrIndexed(assocs) => {
                if let Some(typ) = prefix_typ.sliced_as() {
                    if self
                        .assoc_as_discrete_range_type(scope, assocs, diagnostics)?
                        .is_some()
                    {
                        return Ok(Some(TypeOrMethod::Type(typ)));
                    }
                }

                if could_be_indexed_name(assocs) {
                    // @TODO check types of indexes
                    self.analyze_assoc_elems(scope, assocs, diagnostics)?;
                    if let Some((elem_type, num_indexes)) = prefix_typ.array_type() {
                        if assocs.len() != num_indexes {
                            Err(Diagnostic::dimension_mismatch(
                                name_pos,
                                prefix_typ,
                                assocs.len(),
                                num_indexes,
                            )
                            .into())
                        } else {
                            Ok(Some(TypeOrMethod::Type(elem_type)))
                        }
                    } else {
                        Ok(None)
                    }
                } else {
                    Ok(None)
                }
            }
        }
    }

    pub fn name_resolve(
        &self,
        scope: &Scope<'a>,
        name_pos: &SrcPos,
        name: &mut Name,
        diagnostics: &mut dyn DiagnosticHandler,
    ) -> AnalysisResult<Option<ResolvedName<'a>>> {
        self.name_resolve_with_suffixes(scope, name_pos, name, None, false, diagnostics)
    }

    fn name_resolve_with_suffixes(
        &self,
        scope: &Scope<'a>,
        name_pos: &SrcPos,
        name: &mut Name,
        ttyp: Option<TypeEnt<'a>>,
        has_suffix: bool,
        diagnostics: &mut dyn DiagnosticHandler,
    ) -> AnalysisResult<Option<ResolvedName<'a>>> {
        let mut suffix;
        let prefix;
        let mut resolved = match SplitName::from_name(name) {
            SplitName::Designator(designator) => {
                let name = scope.lookup(name_pos, designator.designator())?;
                return Ok(Some(match name {
                    NamedEntities::Single(ent) => {
                        designator.set_unique_reference(ent);
                        ResolvedName::from_scope_not_overloaded(ent)
                            .map_err(|e| Diagnostic::error(name_pos, e))?
                    }
                    NamedEntities::Overloaded(overloaded) => ResolvedName::Overloaded(
                        WithPos::new(designator.item.clone(), name_pos.clone()),
                        overloaded,
                    ),
                }));
            }
            SplitName::External(ename) => {
                let ExternalName { subtype, class, .. } = ename;
                let subtype = self.resolve_subtype_indication(scope, subtype, diagnostics)?;
                return Ok(Some(ResolvedName::ObjectName(ObjectName {
                    base: ObjectBase::ExternalName(*class),
                    type_mark: Some(subtype.type_mark().to_owned()),
                })));
            }
            SplitName::Suffix(p, s) => {
                let resolved = try_unknown!(self.name_resolve_with_suffixes(
                    scope,
                    &p.pos,
                    &mut p.item,
                    None,
                    true,
                    diagnostics
                ));
                prefix = p;
                suffix = s;
                resolved
            }
        };

        // Any other suffix must collapse overloaded
        if !matches!(suffix, Suffix::CallOrIndexed(_)) {
            if let ResolvedName::Overloaded(ref des, ref overloaded) = resolved {
                let disambiguated = self.disambiguate_no_actuals(
                    des,
                    {
                        // @TODO must be disambiguated with suffixes
                        None
                    },
                    overloaded,
                )?;

                if let Some(disambiguated) = disambiguated {
                    match disambiguated {
                        Disambiguated::Ambiguous(_) => {
                            // @TODO ambiguous error
                            return Ok(None);
                        }
                        Disambiguated::Unambiguous(ent) => {
                            if let Some(typ) = ent.return_type() {
                                resolved =
                                    ResolvedName::Expression(DisambiguatedType::Unambiguous(typ));
                            } else {
                                diagnostics.error(
                                    &prefix.pos,
                                    "Procedure calls are not valid in names and expressions",
                                );
                                return Ok(None);
                            }
                        }
                    }
                }
            }
        }

        if let Suffix::Attribute(attr) = suffix {
            if let Some(signature) = attr.signature {
                if let Err(e) = self.resolve_signature(scope, signature) {
                    diagnostics.push(e.into_non_fatal()?);
                }
            }
            if let Some(expr) = attr.expr {
                self.analyze_expression(scope, expr, diagnostics)?;
            }

            // @TODO not handled yet
            return Ok(None);
        }

        match resolved {
            ResolvedName::Overloaded(ref des, ref overloaded) => {
                if let Suffix::CallOrIndexed(ref mut assocs) = suffix {
                    // @TODO could be overloaded with no arguments that is indexed

                    // @TODO lookup already set reference to get O(N) instead of O(N^2) when disambiguating deeply nested ambiguous calls
                    if let Some(id) = prefix.item.get_suffix_reference() {
                        if let Ok(ent) = OverloadedEnt::from_any(self.arena.get(id)) {
                            return Ok(Some(ResolvedName::Expression(
                                DisambiguatedType::Unambiguous(ent.return_type().unwrap()),
                            )));
                        }
                    }

                    match self.disambiguate(
                        scope,
                        name_pos,
                        des,
                        assocs,
                        if has_suffix {
                            // @TODO disambiguate based on suffixes
                            None
                        } else {
                            ttyp
                        },
                        overloaded.entities().collect(),
                        diagnostics,
                    )? {
                        Some(Disambiguated::Ambiguous(_)) => {
                            // @TODO ambiguous error
                            return Ok(None);
                        }
                        Some(Disambiguated::Unambiguous(ent)) => {
                            prefix.set_unique_reference(&ent);
                            if let Some(return_type) = ent.return_type() {
                                resolved = ResolvedName::Expression(
                                    DisambiguatedType::Unambiguous(return_type),
                                );
                            } else {
                                diagnostics.error(
                                    &prefix.pos,
                                    "Procedure calls are not valid in names and expressions",
                                );
                                return Ok(None);
                            }
                        }
                        None => {
                            return Ok(None);
                        }
                    }
                } else {
                    diagnostics.push(Diagnostic::unreachable(
                        name_pos,
                        "CallOrIndexed should already be handled",
                    ));
                    return Ok(None);
                }
            }
            ResolvedName::ObjectName(oname) => {
                match self.resolve_typed_suffix(
                    scope,
                    &prefix.pos,
                    name_pos,
                    oname.type_mark(),
                    &mut suffix,
                    diagnostics,
                )? {
                    Some(TypeOrMethod::Type(typ)) => {
                        resolved = ResolvedName::ObjectName(oname.with_suffix(typ));
                    }
                    Some(TypeOrMethod::Method(des, name)) => {
                        resolved = ResolvedName::Overloaded(des, name);
                    }
                    None => {
                        return Err(
                            Diagnostic::cannot_be_prefix(&prefix.pos, resolved, suffix).into()
                        );
                    }
                }
            }
            ResolvedName::Expression(ref typ) => match typ {
                DisambiguatedType::Unambiguous(typ) => {
                    match self.resolve_typed_suffix(
                        scope,
                        &prefix.pos,
                        name_pos,
                        *typ,
                        &mut suffix,
                        diagnostics,
                    )? {
                        Some(TypeOrMethod::Type(typ)) => {
                            resolved =
                                ResolvedName::Expression(DisambiguatedType::Unambiguous(typ));
                        }
                        Some(TypeOrMethod::Method(des, name)) => {
                            resolved = ResolvedName::Overloaded(des, name);
                        }
                        None => {
                            return Err(Diagnostic::cannot_be_prefix(
                                &prefix.pos,
                                resolved,
                                suffix,
                            )
                            .into());
                        }
                    }
                }
                DisambiguatedType::Ambiguous(_) => {
                    // @TODO ambiguous error
                    return Ok(None);
                }
            },

            ResolvedName::Library(ref library_name) => {
                if let Suffix::Selected(ref mut designator) = suffix {
                    resolved = ResolvedName::Design(self.lookup_in_library(
                        library_name,
                        &designator.pos,
                        &designator.item.item,
                        &mut designator.item.reference,
                    )?);
                } else {
                    return Err(Diagnostic::cannot_be_prefix(name_pos, resolved, suffix).into());
                }
            }
            ResolvedName::Design(ref ent) => {
                if let Suffix::Selected(ref mut designator) = suffix {
                    let name = ent.selected(&prefix.pos, designator)?;
                    designator.set_reference(&name);
                    resolved = match name {
                        NamedEntities::Single(named_entity) => {
                            ResolvedName::from_design_not_overloaded(named_entity)
                                .map_err(|e| Diagnostic::error(&designator.pos, e))?
                        }
                        NamedEntities::Overloaded(overloaded) => {
                            // Could be used for an alias of a subprogram
                            ResolvedName::Overloaded(
                                WithPos::new(designator.item.item.clone(), designator.pos.clone()),
                                overloaded,
                            )
                        }
                    }
                } else {
                    return Err(Diagnostic::cannot_be_prefix(name_pos, resolved, suffix).into());
                }
            }
            ResolvedName::Type(typ) => {
                if let Suffix::CallOrIndexed(ref assocs) = suffix {
                    if assocs.len() == 1 && could_be_indexed_name(assocs) {
                        // @TODO Type conversion, check argument
                        return Ok(Some(ResolvedName::Expression(
                            DisambiguatedType::Unambiguous(typ),
                        )));
                    }
                }
                return Err(Diagnostic::cannot_be_prefix(name_pos, resolved, suffix).into());
            }
            ResolvedName::Final(_) => {
                return Err(Diagnostic::cannot_be_prefix(name_pos, resolved, suffix).into());
            }
        }

        Ok(Some(resolved))
    }
    // Helper function:
    // Resolve a name that must be some kind of object selection, index or slice
    // Such names occur as assignment targets and aliases
    // Takes an error message as an argument to be re-usable
    pub fn resolve_object_name(
        &self,
        scope: &Scope<'a>,
        name_pos: &SrcPos,
        name: &mut Name,
        err_msg: &'static str,
        diagnostics: &mut dyn DiagnosticHandler,
    ) -> AnalysisResult<Option<ObjectName<'a>>> {
        let resolved = try_unknown!(self.name_resolve(scope, name_pos, name, diagnostics));
        match resolved {
            ResolvedName::ObjectName(oname) => Ok(Some(oname)),
            ResolvedName::Library(_)
            | ResolvedName::Design(_)
            | ResolvedName::Type(_)
            | ResolvedName::Overloaded { .. }
            | ResolvedName::Expression(_)
            | ResolvedName::Final(_) => Err(Diagnostic::error(
                name_pos,
                format!("{} {}", resolved.describe(), err_msg),
            )
            .into()),
        }
    }

    /// Analyze a name that is part of an expression that could be ambiguous
    pub fn expression_name_types(
        &self,
        scope: &Scope<'a>,
        expr_pos: &SrcPos,
        name: &mut Name,
        diagnostics: &mut dyn DiagnosticHandler,
    ) -> FatalResult<Option<DisambiguatedType<'a>>> {
        match self.name_resolve_with_suffixes(scope, expr_pos, name, None, false, diagnostics) {
            Ok(Some(resolved)) => match self.name_to_type(expr_pos, resolved) {
                Ok(Some(typ)) => Ok(Some(typ)),
                Ok(None) => Ok(None),
                Err(diag) => {
                    diagnostics.push(diag);
                    Ok(None)
                }
            },
            Ok(None) => Ok(None),
            Err(err) => {
                diagnostics.push(err.into_non_fatal()?);
                Ok(None)
            }
        }
    }

    /// Analyze a name that is part of an expression that must be unambiguous
    pub fn expression_name_with_ttyp(
        &self,
        scope: &Scope<'a>,
        expr_pos: &SrcPos,
        name: &mut Name,
        ttyp: TypeEnt<'a>,
        diagnostics: &mut dyn DiagnosticHandler,
    ) -> FatalResult {
        match self.name_resolve_with_suffixes(scope, expr_pos, name, Some(ttyp), false, diagnostics)
        {
            Ok(Some(resolved)) => {
                // @TODO target_type already used above, functions could probably be simplified
                match self.name_to_unambiguous_type(
                    expr_pos,
                    &resolved,
                    ttyp,
                    name.suffix_reference_mut(),
                ) {
                    Ok(Some(type_mark)) => {
                        if !self.can_be_target_type(type_mark, ttyp.base()) {
                            diagnostics.push(Diagnostic::type_mismatch(
                                expr_pos,
                                &resolved.describe_type(),
                                ttyp,
                            ));
                        }
                    }
                    Ok(None) => {}
                    Err(diag) => {
                        diagnostics.push(diag);
                    }
                }
            }
            Ok(None) => {}
            Err(err) => {
                diagnostics.push(err.into_non_fatal()?);
            }
        }
        Ok(())
    }

    /// Fallback solution that just lookups names
    pub fn resolve_name(
        &self,
        scope: &Scope<'a>,
        name_pos: &SrcPos,
        name: &mut Name,
        diagnostics: &mut dyn DiagnosticHandler,
    ) -> FatalResult<Option<NamedEntities<'a>>> {
        match name {
            Name::Selected(prefix, suffix) => {
                match self.resolve_name(scope, &prefix.pos, &mut prefix.item, diagnostics)? {
                    Some(NamedEntities::Single(named_entity)) => {
                        match self.lookup_selected(&prefix.pos, named_entity, suffix) {
                            Ok(visible) => {
                                suffix.set_reference(&visible);
                                Ok(Some(visible))
                            }
                            Err(err) => {
                                err.add_to(diagnostics)?;
                                Ok(None)
                            }
                        }
                    }
                    Some(NamedEntities::Overloaded(..)) => Ok(None),
                    None => Ok(None),
                }
            }

            Name::SelectedAll(prefix) => {
                self.resolve_name(scope, &prefix.pos, &mut prefix.item, diagnostics)?;

                Ok(None)
            }
            Name::Designator(designator) => match scope.lookup(name_pos, designator.designator()) {
                Ok(visible) => {
                    designator.set_reference(&visible);
                    Ok(Some(visible))
                }
                Err(diagnostic) => {
                    diagnostics.push(diagnostic);
                    Ok(None)
                }
            },
            Name::Slice(ref mut prefix, ref mut drange) => {
                self.resolve_name(scope, &prefix.pos, &mut prefix.item, diagnostics)?;
                self.analyze_discrete_range(scope, drange.as_mut(), diagnostics)?;
                Ok(None)
            }
            Name::Attribute(ref mut attr) => {
                self.analyze_attribute_name(scope, attr, diagnostics)?;
                Ok(None)
            }
            Name::CallOrIndexed(ref mut call) => {
                self.resolve_name(scope, &call.name.pos, &mut call.name.item, diagnostics)?;
                self.analyze_assoc_elems(scope, &mut call.parameters, diagnostics)?;
                Ok(None)
            }
            Name::External(ref mut ename) => {
                let ExternalName { subtype, .. } = ename.as_mut();
                self.analyze_subtype_indication(scope, subtype, diagnostics)?;
                Ok(None)
            }
        }
    }

    pub fn analyze_attribute_name(
        &self,
        scope: &Scope<'a>,
        attr: &mut AttributeName,
        diagnostics: &mut dyn DiagnosticHandler,
    ) -> FatalResult {
        // @TODO more, attr must be checked inside the scope of attributes of prefix
        let AttributeName {
            name,
            signature,
            expr,
            ..
        } = attr;

        self.resolve_name(scope, &name.pos, &mut name.item, diagnostics)?;

        if let Some(ref mut signature) = signature {
            if let Err(err) = self.resolve_signature(scope, signature) {
                err.add_to(diagnostics)?;
            }
        }
        if let Some(ref mut expr) = expr {
            self.analyze_expression(scope, expr, diagnostics)?;
        }
        Ok(())
    }

    /// Analyze an indexed name where the prefix entity is already known
    /// Returns the type of the array element
    pub fn analyze_indexed_name(
        &self,
        scope: &Scope<'a>,
        name_pos: &SrcPos,
        suffix_pos: &SrcPos,
        type_mark: TypeEnt<'a>,
        indexes: &mut [Index],
        diagnostics: &mut dyn DiagnosticHandler,
    ) -> AnalysisResult<TypeEnt<'a>> {
        let base_type = type_mark.base_type();

        let base_type = if let Type::Access(ref subtype, ..) = base_type.kind() {
            subtype.base_type()
        } else {
            base_type
        };

        if let Type::Array {
            indexes: ref index_types,
            elem_type,
            ..
        } = base_type.kind()
        {
            if indexes.len() != index_types.len() {
                diagnostics.push(Diagnostic::dimension_mismatch(
                    name_pos,
                    base_type,
                    indexes.len(),
                    index_types.len(),
                ))
            }

            for index in indexes.iter_mut() {
                self.analyze_expression_pos(scope, index.pos, index.expr, diagnostics)?;
            }

            Ok(*elem_type)
        } else {
            Err(Diagnostic::error(
                suffix_pos,
                format!("{} cannot be indexed", type_mark.describe()),
            )
            .into())
        }
    }

    pub fn lookup_selected(
        &self,
        prefix_pos: &SrcPos,
        prefix: EntRef<'a>,
        suffix: &mut WithPos<WithRef<Designator>>,
    ) -> AnalysisResult<NamedEntities<'a>> {
        match prefix.actual_kind() {
            AnyEntKind::Library => {
                let library_name = prefix.designator().expect_identifier();
                let named_entity = self.lookup_in_library(
                    library_name,
                    &suffix.pos,
                    &suffix.item.item,
                    &mut suffix.item.reference,
                )?;

                Ok(NamedEntities::new(named_entity.into()))
            }
            AnyEntKind::Object(ref object) => Ok(object
                .subtype
                .type_mark()
                .selected(prefix_pos, suffix)?
                .into_any()),
            AnyEntKind::ObjectAlias { ref type_mark, .. } => {
                Ok(type_mark.selected(prefix_pos, suffix)?.into_any())
            }
            AnyEntKind::ExternalAlias { ref type_mark, .. } => {
                Ok(type_mark.selected(prefix_pos, suffix)?.into_any())
            }
            AnyEntKind::ElementDeclaration(ref subtype) => {
                Ok(subtype.type_mark().selected(prefix_pos, suffix)?.into_any())
            }
            AnyEntKind::Design(_) => {
                let design = DesignEnt::from_any(prefix).map_err(|ent| {
                    Diagnostic::error(
                        &suffix.pos,
                        format!(
                            "Internal error when expecting design unit, got {}",
                            ent.describe()
                        ),
                    )
                })?;

                let named = design.selected(prefix_pos, suffix)?;
                Ok(named)
            }

            _ => Err(Diagnostic::invalid_selected_name_prefix(prefix, prefix_pos).into()),
        }
    }

    pub fn resolve_selected_name(
        &self,
        scope: &Scope<'a>,
        name: &mut WithPos<SelectedName>,
    ) -> AnalysisResult<NamedEntities<'a>> {
        match name.item {
            SelectedName::Selected(ref mut prefix, ref mut suffix) => {
                let prefix_ent = self
                    .resolve_selected_name(scope, prefix)?
                    .into_non_overloaded();
                if let Ok(prefix_ent) = prefix_ent {
                    let visible = self.lookup_selected(&prefix.pos, prefix_ent, suffix)?;
                    suffix.set_reference(&visible);
                    return Ok(visible);
                };

                Err(AnalysisError::NotFatal(Diagnostic::error(
                    &prefix.pos,
                    "Invalid prefix for selected name",
                )))
            }
            SelectedName::Designator(ref mut designator) => {
                let visible = scope.lookup(&name.pos, designator.designator())?;
                designator.set_reference(&visible);
                Ok(visible)
            }
        }
    }
}

fn plural(singular: &'static str, plural: &'static str, count: usize) -> &'static str {
    if count == 1 {
        singular
    } else {
        plural
    }
}

impl Diagnostic {
    fn cannot_be_prefix(prefix_pos: &SrcPos, resolved: ResolvedName, suffix: Suffix) -> Diagnostic {
        let suffix_desc = match suffix {
            Suffix::Selected(_) => "selected",
            Suffix::All => "accessed with .all",
            Suffix::Slice(_) => "sliced",
            Suffix::Attribute(_) => "the prefix of an attribute",
            Suffix::CallOrIndexed(ref assoc) => {
                if could_be_indexed_name(assoc) {
                    "indexed"
                } else {
                    "called as a function"
                }
            }
        };

        let name_desc = if matches!(suffix, Suffix::CallOrIndexed(ref assoc) if !could_be_indexed_name(assoc) )
        {
            // When something cannot be called as a function the type is not relevant
            resolved.describe()
        } else {
            resolved.describe_type()
        };

        Diagnostic::error(
            prefix_pos,
            format!("{name_desc} cannot be {suffix_desc}"),
        )
    }

    fn dimension_mismatch(
        pos: &SrcPos,
        base_type: TypeEnt,
        got: usize,
        expected: usize,
    ) -> Diagnostic {
        let mut diag = Diagnostic::error(pos, "Number of indexes does not match array dimension");

        if let Some(decl_pos) = base_type.decl_pos() {
            diag.add_related(
                decl_pos,
                capitalize(&format!(
                    "{} has {} {}, got {} {}",
                    base_type.describe(),
                    expected,
                    plural("dimension", "dimensions", expected),
                    got,
                    plural("index", "indexes", got),
                )),
            );
        }

        diag
    }

    /// An internal logic error that we want to show to the user to get bug reports
    fn unreachable(pos: &SrcPos, expected: &str) -> Diagnostic {
        Diagnostic::warning(
            pos,
            format!("Internal error, unreachable code {expected}"),
        )
    }

    fn ambiguous_call<'a>(
        call_name: &WithPos<Designator>,
        candidates: impl IntoIterator<Item = OverloadedEnt<'a>>,
    ) -> Diagnostic {
        let mut diag = Diagnostic::error(
            &call_name.pos,
            format!("Ambiguous call to {}", call_name.item.describe()),
        );
        diag.add_subprogram_candidates("Migth be", candidates);
        diag
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use assert_matches::assert_matches;

    use crate::analysis::tests::NoDiagnostics;
    use crate::analysis::tests::TestSetup;
    use crate::syntax::test::check_diagnostics;
    use crate::syntax::test::Code;

    impl<'a> TestSetup<'a> {
        fn name_resolve(
            &'a self,
            code: &Code,
            ttyp: Option<TypeEnt<'a>>,
            diagnostics: &mut dyn DiagnosticHandler,
        ) -> Result<Option<ResolvedName<'a>>, Diagnostic> {
            let mut name = code.name();
            self.ctx()
                .name_resolve_with_suffixes(
                    &self.scope,
                    &name.pos,
                    &mut name.item,
                    ttyp,
                    false,
                    diagnostics,
                )
                .map_err(|e| e.into_non_fatal().unwrap())
        }

        fn expression_name_with_ttyp(
            &'a self,
            code: &Code,
            ttyp: TypeEnt<'a>,
            diagnostics: &mut dyn DiagnosticHandler,
        ) {
            let mut name = code.name();
            self.ctx()
                .expression_name_with_ttyp(
                    &self.scope,
                    &name.pos,
                    &mut name.item,
                    ttyp,
                    diagnostics,
                )
                .unwrap()
        }

        fn expression_name_types(
            &'a self,
            code: &Code,
            diagnostics: &mut dyn DiagnosticHandler,
        ) -> Option<DisambiguatedType<'a>> {
            let mut name = code.name();
            self.ctx()
                .expression_name_types(&self.scope, &name.pos, &mut name.item, diagnostics)
                .unwrap()
        }
    }

    #[test]
    fn object_name() {
        let test = TestSetup::new();
        test.declarative_part("constant c0 : natural := 0;");
        let resolved = test
            .name_resolve(&test.snippet("c0"), None, &mut NoDiagnostics)
            .unwrap();
        assert_matches!(resolved, Some(ResolvedName::ObjectName(_)));
    }

    #[test]
    fn selected_object_name() {
        let test = TestSetup::new();
        test.declarative_part(
            "
type rec_t is record
  field : natural;
end record;
constant c0 : rec_t := (others => 0);
",
        );
        let resolved = test
            .name_resolve(&test.snippet("c0.field"), None, &mut NoDiagnostics)
            .unwrap();
        assert_matches!(resolved, Some(ResolvedName::ObjectName(oname)) if oname.type_mark() == test.lookup_type("natural"));
    }

    #[test]
    fn access_all() {
        let test = TestSetup::new();
        test.declarative_part(
            "
type ptr_t is access integer_vector;
variable vptr : ptr_t;
",
        );
        let resolved = test
            .name_resolve(&test.snippet("vptr.all"), None, &mut NoDiagnostics)
            .unwrap();
        assert_matches!(resolved, Some(ResolvedName::ObjectName(oname)) if oname.type_mark() == test.lookup_type("integer_vector"));
    }

    #[test]
    fn indexed_name() {
        let test = TestSetup::new();
        test.declarative_part(
            "
variable c0 : integer_vector(0 to 1);
",
        );
        let resolved = test
            .name_resolve(&test.snippet("c0(0)"), None, &mut NoDiagnostics)
            .unwrap();
        assert_matches!(resolved, Some(ResolvedName::ObjectName(oname)) if oname.type_mark() == test.lookup_type("integer"));
    }

    #[test]
    fn indexed_name_cannot_be_call() {
        let test = TestSetup::new();
        test.declarative_part(
            "
variable c0 : integer_vector(0 to 1);
",
        );
        let code = test.snippet("c0(open)");
        let resolved = test.name_resolve(&test.snippet("c0(open)"), None, &mut NoDiagnostics);
        assert_eq!(
            resolved,
            Err(Diagnostic::error(
                &code.s1("c0"),
                "variable 'c0' cannot be called as a function"
            ))
        );
    }

    #[test]
    fn overloaded_name() {
        let test = TestSetup::new();
        let resolved = test
            .name_resolve(&test.snippet("true"), None, &mut NoDiagnostics)
            .unwrap();
        assert_matches!(resolved, Some(ResolvedName::Overloaded { .. }));
    }

    #[test]
    fn call_result() {
        let test = TestSetup::new();
        test.declarative_part(
            "
function fun(arg: natural) return integer;
        ",
        );
        let resolved = test
            .name_resolve(&test.snippet("fun(0)"), None, &mut NoDiagnostics)
            .unwrap()
            .unwrap();
        assert_eq!(
            resolved,
            ResolvedName::Expression(DisambiguatedType::Unambiguous(test.lookup_type("integer"))),
        );
    }

    #[test]
    fn disambiguates_call_with_arguments_by_return_type() {
        let test = TestSetup::new();
        test.declarative_part(
            "
function fun(arg: natural) return integer;
function fun(arg: natural) return character;
        ",
        );
        test.expression_name_with_ttyp(
            &test.snippet("fun(0)"),
            test.lookup_type("integer"),
            &mut NoDiagnostics,
        );
    }

    #[test]
    fn overloaded_name_can_be_selected() {
        let test = TestSetup::new();
        test.declarative_part(
            "
type rec_t is record
    fld : natural;
end record;

function foo return rec_t;
",
        );
        test.expression_name_with_ttyp(
            &test.snippet("foo.fld"),
            test.lookup_type("natural"),
            &mut NoDiagnostics,
        );
    }

    #[test]
    fn procedure_cannot_be_used() {
        let test = TestSetup::new();
        test.declarative_part(
            "
procedure proc(arg: natural);
        ",
        );
        let mut diagnostics = Vec::new();
        let code = test.snippet("proc(0)");
        let resolved = test.name_resolve(&code, None, &mut diagnostics).unwrap();
        assert_eq!(resolved, None);
        check_diagnostics(
            diagnostics,
            vec![Diagnostic::error(
                code.s1("proc"),
                "Procedure calls are not valid in names and expressions",
            )],
        );
    }

    #[test]
    fn file_can_be_expression() {
        let test = TestSetup::new();
        test.declarative_part(
            "
type file_t is file of character;
file myfile : file_t;
",
        );
        let code = test.snippet("myfile");
        assert_eq!(
            test.expression_name_types(&code, &mut NoDiagnostics),
            Some(DisambiguatedType::Unambiguous(test.lookup_type("file_t"))),
        )
    }

    #[test]
    fn disambiguates_by_target_type() {
        let test = TestSetup::new();
        test.declarative_part(
            "
type enum1_t is (alpha, beta);
type enum2_t is (alpha, beta);
",
        );
        let code = test.snippet("alpha");
        test.expression_name_with_ttyp(&code, test.lookup_type("enum2_t"), &mut NoDiagnostics);
    }

    #[test]
    fn fcall_result_can_be_sliced() {
        let test = TestSetup::new();
        test.declarative_part(
            "
function myfun(arg : integer) return string;
",
        );
        let code = test.snippet("myfun(0)(0 to 1)");
        assert_eq!(
            test.name_resolve(&code, None, &mut NoDiagnostics),
            Ok(Some(ResolvedName::Expression(
                DisambiguatedType::Unambiguous(test.lookup_type("string"))
            ),))
        );
    }

    #[test]
    fn fcall_without_actuals_can_be_sliced() {
        let test = TestSetup::new();
        test.declarative_part(
            "
function myfun return string;
",
        );
        let code = test.snippet("myfun(0 to 1)");
        assert_eq!(
            test.name_resolve(&code, None, &mut NoDiagnostics),
            Ok(Some(ResolvedName::Expression(
                DisambiguatedType::Unambiguous(test.lookup_type("string"))
            ),))
        );
    }

    #[test]
    fn disambiguates_with_target_type() {
        let test = TestSetup::new();
        test.declarative_part(
            "
type enum1_t is (alpha, beta);
type enum2_t is (alpha, beta);
",
        );
        let code = test.snippet("alpha");
        test.expression_name_with_ttyp(&code, test.lookup_type("enum1_t"), &mut NoDiagnostics);
    }

    #[test]
    fn slice_access_type() {
        let test = TestSetup::new();
        test.declarative_part(
            "
type ptr_t is access integer_vector;
variable vptr : ptr_t; 
",
        );
        let code = test.snippet("vptr(0 to 1)");
        assert_matches!(
            test.name_resolve(&code, None, &mut NoDiagnostics),
            Ok(Some(ResolvedName::ObjectName(oname))) if oname.type_mark() == test.lookup_type("integer_vector")
        );
    }

    #[test]
    fn index_access_type() {
        let test = TestSetup::new();
        test.declarative_part(
            "
type ptr_t is access integer_vector;
variable vptr : ptr_t; 
",
        );
        let code = test.snippet("vptr(0)");
        assert_matches!(
            test.name_resolve(&code, None, &mut NoDiagnostics),
            Ok(Some(ResolvedName::ObjectName(oname))) if oname.type_mark() == test.lookup_type("integer")
        );
    }

    #[test]
    fn slice_with_integer_discrete_range() {
        let test = TestSetup::new();
        test.declarative_part(
            "
subtype sub_t is integer range 0 to 3;
variable c0 : integer_vector(0 to 6);
",
        );
        let code = test.snippet("c0(sub_t)");
        assert_matches!(
            test.name_resolve(&code, None, &mut NoDiagnostics),
            Ok(Some(ResolvedName::ObjectName(oname))) if oname.type_mark() == test.lookup_type("integer_vector")
        );
    }

    #[test]
    fn slice_with_enum_discrete_range() {
        let test = TestSetup::new();
        test.declarative_part(
            "
type enum_t is (a, b, c);
type arr_t is array (enum_t) of character;
subtype sub_t is enum_t range a to b;
variable c0 : arr_t(a to c);
",
        );
        let code = test.snippet("c0(sub_t)");
        assert_matches!(
            test.name_resolve(&code, None, &mut NoDiagnostics),
            Ok(Some(ResolvedName::ObjectName(oname))) if oname.type_mark() == test.lookup_type("arr_t")
        );
    }

    #[test]
    fn slice_with_bad_type() {
        let test = TestSetup::new();
        test.declarative_part(
            "
variable c0 : integer_vector(0 to 6);
",
        );
        let code = test.snippet("c0(real)");
        assert_eq!(
            test.name_resolve(&code, None, &mut NoDiagnostics),
            Err(Diagnostic::error(
                code.s1("real"),
                "real type 'REAL' cannot be used as a discrete range"
            ))
        );
    }
}
