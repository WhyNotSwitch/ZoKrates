use std::fmt;

use derivative::Derivative;

use super::{Span, WithSpan};

#[derive(Derivative)]
#[derivative(PartialOrd, PartialEq, Hash, Eq)]
#[derive(Clone)]
pub struct Parameter<V> {
    #[derivative(PartialEq = "ignore", PartialOrd = "ignore", Hash = "ignore")]
    pub span: Option<Span>,
    pub id: V,
    pub private: bool,
}

impl<V> Parameter<V> {
    pub fn new(v: V, private: bool) -> Self {
        Parameter {
            span: None,
            id: v,
            private,
        }
    }

    pub fn public(v: V) -> Self {
        Self::new(v, false)
    }

    pub fn private(v: V) -> Self {
        Self::new(v, true)
    }
}

impl<V> WithSpan for Parameter<V> {
    fn span(mut self, span: Option<Span>) -> Self {
        self.span = span;
        self
    }

    fn get_span(&self) -> Option<Span> {
        self.span
    }
}

impl<V: fmt::Display> fmt::Display for Parameter<V> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let visibility = if self.private { "private " } else { "" };
        write!(f, "{}{}", visibility, self.id)
    }
}

impl<V: fmt::Debug> fmt::Debug for Parameter<V> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Parameter(variable: {:?})", self.id)
    }
}
