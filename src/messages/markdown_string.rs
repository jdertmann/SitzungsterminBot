use std::fmt;
use std::ops::{Add, AddAssign};

use teloxide::types::UserId;
use teloxide::utils::markdown as md;

#[derive(Debug, Clone, Hash, Default)]
pub struct MarkdownString(String, usize);

impl fmt::Display for MarkdownString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for MarkdownString {
    fn from(value: &str) -> Self {
        Self::from_str(value)
    }
}

impl AddAssign<&MarkdownString> for MarkdownString {
    fn add_assign(&mut self, rhs: &MarkdownString) {
        self.0 += &rhs.0;
        self.1 += &rhs.1;
    }
}

impl AddAssign<&str> for MarkdownString {
    fn add_assign(&mut self, rhs: &str) {
        *self += &MarkdownString::from_str(rhs)
    }
}

impl AddAssign<&String> for MarkdownString {
    fn add_assign(&mut self, rhs: &String) {
        *self += rhs.as_str()
    }
}

impl<T> Add<T> for MarkdownString
where
    MarkdownString: AddAssign<T>,
{
    type Output = MarkdownString;

    fn add(mut self, rhs: T) -> Self::Output {
        self += rhs;
        self
    }
}

macro_rules! impl_methods {
    ($($method_name:ident),*) => {
        $(
            #[allow(unused)]
            pub fn $method_name(&self) -> MarkdownString {
                MarkdownString(md::$method_name(&self.0), self.1)
            }
        )*
    };
}

impl MarkdownString {
    pub fn new() -> MarkdownString {
        MarkdownString(String::new(), 0)
    }

    pub fn into_string(self) -> String {
        self.0
    }

    pub fn from_str(s: &str) -> MarkdownString {
        MarkdownString(md::escape(s), s.len())
    }

    #[allow(unused)]
    pub fn code_block(s: &str) -> MarkdownString {
        MarkdownString(md::code_block(s), s.len())
    }

    pub fn code_inline(s: &str) -> MarkdownString {
        MarkdownString(md::code_inline(s), s.len())
    }

    impl_methods! { bold, italic, blockquote, strike, underline }

    #[allow(unused)]
    pub fn link(&self, link: &str) -> MarkdownString {
        MarkdownString(md::link(link, &self.0), self.1)
    }

    #[allow(unused)]
    pub fn user_mention(&self, user_id: UserId) -> MarkdownString {
        MarkdownString(md::user_mention(user_id, &self.0), self.1)
    }

    #[allow(unused)]
    pub fn len_parsed(&self) -> usize {
        self.1
    }

    pub fn join<'a>(
        items: impl IntoIterator<Item = &'a MarkdownString>,
        sep: &MarkdownString,
    ) -> MarkdownString {
        let mut result = MarkdownString::new();
        for (index, item) in items.into_iter().enumerate() {
            if index > 0 {
                result += sep;
            }
            result += item;
        }

        result
    }
}
