use crate::cgroup::{CgroupIdentity, CgroupPath};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CgroupMatch {
    Exact(CgroupPath),
    Glob(String),
    Ancestor(CgroupPath),
    Not(Box<CgroupMatch>),
}

impl CgroupMatch {
    pub fn parse(s: &str) -> Self {
        if let Some(rest) = s.strip_prefix('!') {
            return CgroupMatch::Not(Box::new(CgroupMatch::parse(rest)));
        }

        if let Some(prefix) = s.strip_suffix("/**") {
            CgroupMatch::Ancestor(CgroupPath::new(prefix))
        } else if s.contains('*') || s.contains('?') {
            CgroupMatch::Glob(s.to_string())
        } else {
            CgroupMatch::Exact(CgroupPath::new(s))
        }
    }

    pub fn matches(&self, id: &CgroupIdentity) -> bool {
        match self {
            CgroupMatch::Exact(p) => &id.path == p,
            CgroupMatch::Ancestor(p) => p.is_ancestor_of(&id.path),
            CgroupMatch::Glob(g) => {
                let path_str = id.path.as_path().to_str().unwrap_or("");
                simple_glob_match(g, path_str)
            }
            CgroupMatch::Not(m) => !m.matches(id),
        }
    }

    pub fn specificity(&self) -> u32 {
        match self {
            CgroupMatch::Exact(_) => 10000,
            CgroupMatch::Ancestor(p) => 1000 + p.segments().count() as u32,
            CgroupMatch::Glob(_) => 100,
            CgroupMatch::Not(m) => m.specificity(),
        }
    }
}

// Minimal wildcard matcher supporting '*' and '?'
fn simple_glob_match(pattern: &str, target: &str) -> bool {
    let p_chars: Vec<char> = pattern.chars().collect();
    let t_chars: Vec<char> = target.chars().collect();

    let mut i = 0;
    let mut j = 0;
    let mut star_idx = None;
    let mut match_idx = 0;

    while i < t_chars.len() {
        if j < p_chars.len() && (p_chars[j] == '?' || p_chars[j] == t_chars[i]) {
            i += 1;
            j += 1;
        } else if j < p_chars.len() && p_chars[j] == '*' {
            star_idx = Some(j);
            match_idx = i;
            j += 1;
        } else if let Some(star) = star_idx {
            j = star + 1;
            match_idx += 1;
            i = match_idx;
        } else {
            return false;
        }
    }

    while j < p_chars.len() && p_chars[j] == '*' {
        j += 1;
    }

    j == p_chars.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ident(path: &str) -> CgroupIdentity {
        CgroupIdentity {
            path: CgroupPath::new(path),
        }
    }

    #[test]
    fn test_parse_exact() {
        assert_eq!(
            CgroupMatch::parse("/user.slice/foo.scope"),
            CgroupMatch::Exact(CgroupPath::new("/user.slice/foo.scope"))
        );
    }

    #[test]
    fn test_parse_ancestor() {
        assert_eq!(
            CgroupMatch::parse("/user.slice/**"),
            CgroupMatch::Ancestor(CgroupPath::new("/user.slice"))
        );
    }

    #[test]
    fn test_parse_glob() {
        assert_eq!(
            CgroupMatch::parse("/user.slice/*.scope"),
            CgroupMatch::Glob("/user.slice/*.scope".to_string())
        );
    }

    #[test]
    fn test_parse_not() {
        assert_eq!(
            CgroupMatch::parse("!/user.slice/**"),
            CgroupMatch::Not(Box::new(CgroupMatch::Ancestor(CgroupPath::new(
                "/user.slice"
            ))))
        );
    }

    #[test]
    fn test_match_exact() {
        let m = CgroupMatch::parse("/user.slice/foo.scope");
        assert!(m.matches(&ident("/user.slice/foo.scope")));
        assert!(!m.matches(&ident("/user.slice/bar.scope")));
    }

    #[test]
    fn test_match_ancestor() {
        let m = CgroupMatch::parse("/user.slice/**");
        assert!(m.matches(&ident("/user.slice")));
        assert!(m.matches(&ident("/user.slice/foo.scope")));
        assert!(m.matches(&ident("/user.slice/nested/bar.scope")));
        assert!(!m.matches(&ident("/system.slice/foo.scope")));
    }

    #[test]
    fn test_match_glob() {
        let m = CgroupMatch::parse("/user.slice/app-*.scope");
        assert!(m.matches(&ident("/user.slice/app-foo.scope")));
        assert!(m.matches(&ident("/user.slice/app-.scope")));
        assert!(!m.matches(&ident("/user.slice/session-foo.scope")));
        assert!(!m.matches(&ident("/system.slice/app-foo.scope")));

        let m2 = CgroupMatch::parse("/user.slice/app-?.scope");
        assert!(m2.matches(&ident("/user.slice/app-a.scope")));
        assert!(!m2.matches(&ident("/user.slice/app-ab.scope")));
    }

    #[test]
    fn test_match_not() {
        let m = CgroupMatch::parse("!/user.slice/**");
        assert!(!m.matches(&ident("/user.slice/foo.scope")));
        assert!(m.matches(&ident("/system.slice/foo.scope")));
    }

    #[test]
    fn test_specificity() {
        let exact = CgroupMatch::parse("/user.slice/foo.scope");
        let ancestor_shallow = CgroupMatch::parse("/user.slice/**");
        let ancestor_deep = CgroupMatch::parse("/user.slice/app.slice/**");
        let glob = CgroupMatch::parse("/user.slice/*.scope");
        let not_exact = CgroupMatch::parse("!/user.slice/foo.scope");

        assert_eq!(exact.specificity(), 10000);
        assert_eq!(not_exact.specificity(), 10000);
        assert_eq!(glob.specificity(), 100);

        // Shallow ancestor has 1 segment ("user.slice")
        assert_eq!(ancestor_shallow.specificity(), 1001);

        // Deep ancestor has 2 segments ("user.slice", "app.slice")
        assert_eq!(ancestor_deep.specificity(), 1002);

        assert!(exact.specificity() > ancestor_deep.specificity());
        assert!(ancestor_deep.specificity() > ancestor_shallow.specificity());
        assert!(ancestor_shallow.specificity() > glob.specificity());
    }
}
