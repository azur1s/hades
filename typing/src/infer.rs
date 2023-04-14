use std::collections::HashMap;
use chumsky::span::SimpleSpan;
use syntax::{
    expr::{
        Lit, UnaryOp, BinaryOp,
        Expr,
    },
    ty::*,
};

use super::typed::TExpr;

#[derive(Clone, Debug)]
struct Infer<'src> {
    env: HashMap<&'src str, Type>,
    subst: Vec<Type>,
    constraints: Vec<(Type, Type)>,
}

impl<'src> Infer<'src> {
    fn new() -> Self {
        Infer {
            env: HashMap::new(),
            subst: Vec::new(),
            constraints: Vec::new(),
        }
    }

    /// Generate a fresh type variable
    fn fresh(&mut self) -> Type {
        let i = self.subst.len();
        self.subst.push(Type::Var(i));
        Type::Var(i)
    }

    /// Get a substitution for a type variable
    fn subst(&self, i: usize) -> Option<Type> {
        self.subst.get(i).cloned()
    }

    /// Check if a type variable occurs in a type
    fn occurs(&self, i: usize, t: Type) -> bool {
        use Type::*;
        match t {
            Unit | Bool | Num | Str => false,
            Var(j) => {
                if let Some(t) = self.subst(j) {
                    if t != Var(j) {
                        return self.occurs(i, t);
                    }
                }
                i == j
            },
            Func(args, ret) => {
                args.into_iter().any(|t| self.occurs(i, t)) || self.occurs(i, *ret)
            },
            Tuple(tys) => tys.into_iter().any(|t| self.occurs(i, t)),
            Array(ty) => self.occurs(i, *ty),
        }
    }

    /// Unify two types
    fn unify(&mut self, t1: Type, t2: Type) -> Result<(), String> {
        use Type::*;
        match (t1, t2) {
            // Literal types
            (Unit, Unit)
            | (Bool, Bool)
            | (Num, Num)
            | (Str, Str) => Ok(()),

            // Variable
            (Var(i), Var(j)) if i == j => Ok(()), // Same variables can be unified
            (Var(i), t2) => {
                // If the substitution is not the variable itself,
                // unify the substitution with t2
                if let Some(t) = self.subst(i) {
                    if t != Var(i) {
                        return self.unify(t, t2);
                    }
                }
                // If the variable occurs in t2
                if self.occurs(i, t2.clone()) {
                    return Err(format!("Infinite type: '{} = {}", itoa(i), t2));
                }
                // Set the substitution
                self.subst[i] = t2;
                Ok(())
            },
            (t1, Var(i)) => {
                if let Some(t) = self.subst(i) {
                    if t != Var(i) {
                        return self.unify(t1, t);
                    }
                }
                if self.occurs(i, t1.clone()) {
                    return Err(format!("Infinite type: '{} = {}", itoa(i), t1));
                }
                self.subst[i] = t1;
                Ok(())
            },

            // Function
            (Func(a1, r1), Func(a2, r2)) => {
                // Check the number of arguments
                if a1.len() != a2.len() {
                    return Err(format!("Function argument mismatch: {} != {}", a1.len(), a2.len()));
                }
                // Unify the arguments
                for (a1, a2) in a1.into_iter().zip(a2.into_iter()) {
                    self.unify(a1, a2)?;
                }
                // Unify the return types
                self.unify(*r1, *r2)
            },

            // Tuple
            (Tuple(t1), Tuple(t2)) => {
                // Check the number of elements
                if t1.len() != t2.len() {
                    return Err(format!("Tuple element mismatch: {} != {}", t1.len(), t2.len()));
                }
                // Unify the elements
                for (t1, t2) in t1.into_iter().zip(t2.into_iter()) {
                    self.unify(t1, t2)?;
                }
                Ok(())
            },

            // Array
            (Array(t1), Array(t2)) => self.unify(*t1, *t2),

            // The rest will be type mismatch
            (t1, t2) => Err(format!("Type mismatch: {} != {}", t1, t2)),
        }
    }

    /// Solve the constraints by unifying them
    fn solve(&mut self) -> Result<(), String> {
        for (t1, t2) in self.constraints.clone().into_iter() {
            self.unify(t1, t2)?;
        }
        Ok(())
    }

    /// Substitute the type variables with the substitutions
    fn substitute(&mut self, t: Type) -> Type {
        use Type::*;
        match t {
            // Only match any type that can contain type variables
            Var(i) => {
                if let Some(t) = self.subst(i) {
                    if t != Var(i) {
                        return self.substitute(t);
                    }
                }
                Var(i)
            },
            Func(args, ret) => {
                Func(
                    args.into_iter().map(|t| self.substitute(t)).collect(),
                    Box::new(self.substitute(*ret)),
                )
            },
            Tuple(tys) => Tuple(tys.into_iter().map(|t| self.substitute(t)).collect()),
            Array(ty) => Array(Box::new(self.substitute(*ty))),
            // The rest will be returned as is
            _ => t,
        }
    }

    /// Find a type variable in (typed) expression and substitute them
    fn substitute_texp(&mut self, e: TExpr<'src>) -> TExpr<'src> {
        use TExpr::*;
        match e {
            Lit(_) | Ident(_) => e,
            Unary { op, expr: (e, lspan), ret_ty } => {
                Unary {
                    op,
                    expr: (Box::new(self.substitute_texp(*e)), lspan),
                    ret_ty,
                }
            },
            Binary { op, lhs: (lhs, lspan), rhs: (rhs, rspan), ret_ty } => {
                let lhst = self.substitute_texp(*lhs);
                let rhst = self.substitute_texp(*rhs);
                Binary {
                    op,
                    lhs: (Box::new(lhst), lspan),
                    rhs: (Box::new(rhst), rspan),
                    ret_ty: self.substitute(ret_ty),
                }
            },
            Lambda { params, body: (body, bspan), ret_ty } => {
                let bodyt = self.substitute_texp(*body);
                let paramst = params.into_iter()
                    .map(|(name, ty)| (name, self.substitute(ty)))
                    .collect::<Vec<_>>();
                Lambda {
                    params: paramst,
                    body: (Box::new(bodyt), bspan),
                    ret_ty: self.substitute(ret_ty),
                }
            },
            Call { func: (func, fspan), args } => {
                let funct = self.substitute_texp(*func);
                let argst = args.into_iter()
                    .map(|(arg, span)| (self.substitute_texp(arg), span))
                    .collect::<Vec<_>>();
                Call {
                    func: (Box::new(funct), fspan),
                    args: argst,
                }
            },
            If { cond: (cond, cspan), t: (t, tspan), f: (f, fspan), br_ty } => {
                let condt = self.substitute_texp(*cond);
                let tt = self.substitute_texp(*t);
                let ft = self.substitute_texp(*f);
                If {
                    cond: (Box::new(condt), cspan),
                    t: (Box::new(tt), tspan),
                    f: (Box::new(ft), fspan),
                    br_ty,
                }
            },
            Let { name, ty, value: (v, vspan), body: (b, bspan) } => {
                let vt = self.substitute_texp(*v);
                let bt = self.substitute_texp(*b);
                Let {
                    name,
                    ty: self.substitute(ty),
                    value: (Box::new(vt), vspan),
                    body: (Box::new(bt), bspan),
                }
            },
            Define { name, ty, value: (v, vspan) } => {
                let vt = self.substitute_texp(*v);
                Define {
                    name,
                    ty: self.substitute(ty),
                    value: (Box::new(vt), vspan),
                }
            },
            Block { exprs, void, ret_ty } => {
                let exprst = exprs.into_iter()
                    .map(|(e, span)| (self.substitute_texp(e), span))
                    .collect::<Vec<_>>();
                Block {
                    exprs: exprst,
                    void,
                    ret_ty,
                }
            },
        }
    }

    /// Infer the type of an expression
    fn infer(&mut self, e: Expr<'src>, expected: Type) -> Result<TExpr<'src>, String> {
        match e {
            // Literal values
            // Push the constraint (expected type to be the literal type) and
            // return the typed expression
            Expr::Lit(l) => {
                let t = match l {
                    Lit::Unit => Type::Unit,
                    Lit::Bool(_) => Type::Bool,
                    Lit::Num(_) => Type::Num,
                    Lit::Str(_) => Type::Str,
                };
                self.constraints.push((expected, t));
                Ok(TExpr::Lit(l))
            },

            // Identifiers
            // The same as literals but the type is looked up in the environment
            Expr::Ident(ref x) => {
                let t = self.env.get(x)
                    .ok_or(format!("Unbound variable: {}", x))?;
                self.constraints.push((expected, t.clone()));
                Ok(TExpr::Ident(x.clone()))
            }

            // Unary & binary operators
            // The type of the left and right hand side are inferred and
            // the expected type is determined by the operator
            Expr::Unary(op, (expr, espan)) => match op {
                // Numeric operators (Num -> Num)
                UnaryOp::Neg => {
                    let et = self.infer(*expr, Type::Num)?;
                    self.constraints.push((expected, Type::Num));
                    Ok(TExpr::Unary {
                        op,
                        expr: (Box::new(et), espan),
                        ret_ty: Type::Num,
                    })
                },
                // Boolean operators (Bool -> Bool)
                UnaryOp::Not => {
                    let et = self.infer(*expr, Type::Bool)?;
                    self.constraints.push((expected, Type::Bool));
                    Ok(TExpr::Unary {
                        op,
                        expr: (Box::new(et), espan),
                        ret_ty: Type::Bool,
                    })
                },
            }
            Expr::Binary(op, (lhs, lspan), (rhs, rspan)) => match op {
                // Numeric operators (Num -> Num -> Num)
                BinaryOp::Add
                | BinaryOp::Sub
                | BinaryOp::Mul
                | BinaryOp::Div
                | BinaryOp::Rem
                => {
                    let lt = self.infer(*lhs, Type::Num)?;
                    let rt = self.infer(*rhs, Type::Num)?;
                    self.constraints.push((expected, Type::Num));
                    Ok(TExpr::Binary {
                        op,
                        lhs: (Box::new(lt), lspan),
                        rhs: (Box::new(rt), rspan),
                        ret_ty: Type::Num,
                    })
                },
                // Boolean operators (Bool -> Bool -> Bool)
                BinaryOp::And
                | BinaryOp::Or
                => {
                    let lt = self.infer(*lhs, Type::Bool)?;
                    let rt = self.infer(*rhs, Type::Bool)?;
                    self.constraints.push((expected, Type::Bool));
                    Ok(TExpr::Binary {
                        op,
                        lhs: (Box::new(lt), lspan),
                        rhs: (Box::new(rt), rspan),
                        ret_ty: Type::Bool,
                    })
                },
                // Comparison operators ('a -> 'a -> Bool)
                BinaryOp::Eq
                | BinaryOp::Ne
                | BinaryOp::Lt
                | BinaryOp::Le
                | BinaryOp::Gt
                | BinaryOp::Ge
                => {
                    // Create a fresh type variable and then use it as the
                    // expected type for both the left and right hand side
                    // so the type on both side have to be the same
                    let t = self.fresh();
                    let lt = self.infer(*lhs, t.clone())?;
                    let rt = self.infer(*rhs, t)?;
                    self.constraints.push((expected, Type::Bool));
                    Ok(TExpr::Binary {
                        op,
                        lhs: (Box::new(lt), lspan),
                        rhs: (Box::new(rt), rspan),
                        ret_ty: Type::Bool,
                    })
                },
            }

            // Lambda
            Expr::Lambda(args, ret, (b, bspan)) => {
                // Get the return type or create a fresh type variable
                let rt = ret.unwrap_or(self.fresh());
                // Fill in the type of the arguments with a fresh type
                let xs = args.into_iter()
                    .map(|(x, t)| (x, t.unwrap_or(self.fresh())))
                    .collect::<Vec<_>>();

                // Create a new environment, and add the arguments to it
                // and use the new environment to infer the body
                let mut env = self.env.clone();
                xs.clone().into_iter().for_each(|(x, t)| { env.insert(x, t); });
                let mut inf = self.clone();
                inf.env = env;
                let bt = inf.infer(*b, rt.clone())?;

                // Add the substitutions & constraints from the body
                // if it doesn't already exist
                for s in inf.subst {
                    if !self.subst.contains(&s) {
                        self.subst.push(s);
                    }
                }
                for c in inf.constraints {
                    if !self.constraints.contains(&c) {
                        self.constraints.push(c);
                    }
                }

                // Push the constraints
                self.constraints.push((expected, Type::Func(
                    xs.clone().into_iter()
                        .map(|x| x.1)
                        .collect(),
                    Box::new(rt.clone()),
                )));

                Ok(TExpr::Lambda {
                    params: xs,
                    body: (Box::new(bt), bspan),
                    ret_ty: rt,
                })
            },

            // Call
            Expr::Call((f, fspan), args) => {
                // Generate fresh types for the arguments
                let freshes = args.clone().into_iter()
                    .map(|_| self.fresh())
                    .collect::<Vec<Type>>();
                // Create a function type
                let fsig = Type::Func(
                    freshes.clone(),
                    Box::new(expected),
                );
                // Expect the function to have the function type
                let ft = self.infer(*f, fsig)?;
                // Infer the arguments
                let xs = args.into_iter()
                    .zip(freshes.into_iter())
                    .map(|((x, xspan), t)| {
                        let xt = self.infer(x, t)?;
                        Ok((xt, xspan))
                    })
                    .collect::<Result<Vec<_>, String>>()?;

                Ok(TExpr::Call {
                    func: (Box::new(ft), fspan),
                    args: xs,
                })
            },

            // If
            Expr::If { cond: (c, cspan), t: (t, tspan), f: (f, fspan) } => {
                // Condition has to be a boolean
                let ct = self.infer(*c, Type::Bool)?;
                // The type of the if expression is the same as the
                // expected type
                let tt = self.infer(*t, expected.clone())?;
                let et = self.infer(*f, expected.clone())?;

                Ok(TExpr::If {
                    cond: (Box::new(ct), cspan),
                    t: (Box::new(tt), tspan),
                    f: (Box::new(et), fspan),
                    br_ty: expected,
                })
            },

            // Let & define
            Expr::Let { name, ty, value: (v, vspan), body: (b, bspan) } => {
                // Infer the type of the value
                let ty = ty.unwrap_or(self.fresh());
                let vt = self.infer(*v, ty.clone())?;

                // Create a new environment and add the binding to it
                // and then use the new environment to infer the body
                let mut env = self.env.clone();
                env.insert(name.clone(), ty.clone());
                let mut inf = Infer::new();
                inf.env = env;
                let bt = inf.infer(*b, expected)?;

                Ok(TExpr::Let {
                    name, ty,
                    value: (Box::new(vt), vspan),
                    body: (Box::new(bt), bspan),
                })
            },
            Expr::Define { name, ty, value: (v, vspan) } => {
                let ty = ty.unwrap_or(self.fresh());
                let vt = self.infer(*v, ty.clone())?;
                self.env.insert(name.clone(), ty.clone());

                // Define always returns unit
                self.constraints.push((expected, Type::Unit));

                Ok(TExpr::Define {
                    name, ty,
                    value: (Box::new(vt), vspan),
                })
            },

            // Block
            Expr::Block { exprs, void } => {
                // Infer the type of each expression
                let xs = exprs.into_iter()
                    .map(|(x, xspan)| {
                        let xt = self.infer(*x, expected.clone())?;
                        Ok((xt, xspan))
                    })
                    .collect::<Result<Vec<_>, String>>()?;

                let ret_ty = if void {
                    Type::Unit
                } else {
                    expected
                };

                Ok(TExpr::Block {
                    exprs: xs,
                    void, ret_ty,
                })
            },
        }
    }
}

/// Infer a list of expressions
pub fn infer_exprs(es: Vec<(Expr, SimpleSpan)>) -> (Vec<(TExpr, SimpleSpan)>, String) {
    let mut inf = Infer::new();
    // Typed expressions
    let mut tes = vec![];
    // Typed expressions without substitutions
    let mut tes_nosub = vec![];
    // Errors
    let mut errs = vec![];

    for e in es {
        let f = inf.fresh();
        let t = inf.infer(e.0, f).unwrap();
        tes.push(Some((t.clone(), e.1)));
        tes_nosub.push((t, e.1));

        match inf.solve() {
            Ok(_) => {
                // Substitute the type variables for the solved expressions
                tes = tes.into_iter()
                    .map(|te| match te {
                        Some((t, s)) => {
                            Some((inf.substitute_texp(t), s))
                        },
                        None => None,
                    })
                    .collect();
            },
            Err(e) => {
                errs.push(e);
                // Replace the expression with None
                tes.pop();
                tes.push(None);
            },
        }
    }

    // Union typed expressions, replacing None with the typed expression without substitutions
    // None means that the expression has an error
    let mut tes_union = vec![];
    for (te, te_nosub) in tes.into_iter().zip(tes_nosub.into_iter()) {
        match te {
            Some(t) => {
                tes_union.push(t);
            },
            None => {
                tes_union.push(te_nosub);
            },
        }
    }

    (
        // Renamer::new().process(tes_union),
        tes_union,
        errs.join("\n")
    )
}