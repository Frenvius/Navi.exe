pub struct Facts {
    pub header: bool,
    pub tree: bool,
    pub upper: bool,
    pub share: bool,
    pub properties: bool,
    pub status: bool,
    pub back: bool,
    pub forward: bool,
    pub up: bool,
    pub settings: bool,
    pub update: bool,
}

#[derive(Clone, Copy)]
pub enum Step {
    Tab,
    ReverseTab,
    Pane,
    ReversePane,
}

pub fn destination(at: &str, facts: &Facts, step: Step) -> &'static str {
    let Facts {
        header,
        tree,
        upper,
        share,
        properties: props,
        ..
    } = *facts;
    let reverse = matches!(step, Step::ReverseTab | Step::ReversePane);
    let fast = matches!(step, Step::Pane | Step::ReversePane);
    let first_pane = if share {
        Some("pane-share")
    } else if props {
        Some("pane-properties")
    } else {
        None
    };
    if at == "neutral" {
        return if reverse {
            if facts.status {
                "status"
            } else {
                first_pane.unwrap_or(if header { "header" } else { "list" })
            }
        } else if tree {
            "tree"
        } else {
            "list"
        };
    }
    let destination = if fast {
        if reverse {
            match at {
                "list" => {
                    if tree {
                        "tree"
                    } else if upper {
                        "tab"
                    } else {
                        "status"
                    }
                }
                "header" => "list",
                "status" => first_pane.unwrap_or(if header { "header" } else { "list" }),
                "pane-share" | "pane-properties" => {
                    if tree {
                        "tree"
                    } else {
                        "list"
                    }
                }
                "tree" | "crumbs" => {
                    if upper {
                        "tab"
                    } else {
                        "status"
                    }
                }
                _ => "status",
            }
        } else {
            match at {
                "list" => {
                    if header {
                        "header"
                    } else {
                        "status"
                    }
                }
                "header" | "pane-share" | "pane-properties" => "status",
                "status" => {
                    if upper {
                        "tab"
                    } else if tree {
                        "tree"
                    } else {
                        "list"
                    }
                }
                "tree" => "list",
                _ => {
                    if tree {
                        "tree"
                    } else {
                        "list"
                    }
                }
            }
        }
    } else {
        let mut ring = Vec::new();
        if upper {
            ring.extend(["tab", "new-tab"]);
            if facts.back {
                ring.push("back");
            }
            if facts.forward {
                ring.push("forward");
            }
            if facts.up {
                ring.push("up");
            }
            ring.extend(["refresh", "address", "crumbs", "search"]);
            if facts.settings {
                ring.push("settings");
            }
            if facts.update {
                ring.push("update");
            }
            ring.extend(["commands", "pane-command"]);
        }
        if tree {
            ring.push("tree");
        }
        ring.push("list");
        if header {
            ring.push("header");
        }
        if share {
            ring.push("pane-share");
        }
        if props {
            ring.push("pane-properties");
        }
        if facts.status {
            ring.push("status");
        }
        let i = ring
            .iter()
            .position(|&z| z == at)
            .unwrap_or_else(|| ring.iter().position(|&z| z == "list").unwrap());
        if reverse && at == "status" && first_pane.is_some() {
            first_pane.unwrap()
        } else {
            ring[(i as isize + if reverse { -1 } else { 1 }).rem_euclid(ring.len() as isize)
                as usize]
        }
    };
    let destination = if destination == "status" && !facts.status {
        if reverse {
            first_pane.unwrap_or(if header { "header" } else { "list" })
        } else if upper {
            "tab"
        } else if tree {
            "tree"
        } else {
            "list"
        }
    } else {
        destination
    };
    if destination == "commands" && reverse && !fast {
        "commands-last"
    } else {
        destination
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> Facts {
        Facts {
            header: true,
            tree: true,
            upper: true,
            share: false,
            properties: false,
            status: true,
            back: false,
            forward: false,
            up: true,
            settings: false,
            update: false,
        }
    }

    #[test]
    fn measured_details_tab_ring_skips_disabled_history_and_enters_command_groups_directionally() {
        let facts = facts();
        let expected = [
            "list",
            "header",
            "status",
            "tab",
            "new-tab",
            "up",
            "refresh",
            "address",
            "crumbs",
            "search",
            "commands",
            "pane-command",
            "tree",
            "list",
        ];
        for pair in expected.windows(2) {
            assert_eq!(destination(pair[0], &facts, Step::Tab), pair[1]);
        }
        assert_eq!(
            destination("pane-command", &facts, Step::ReverseTab),
            "commands-last"
        );
        assert_eq!(destination("neutral", &facts, Step::Tab), "tree");
        assert_eq!(destination("neutral", &facts, Step::ReversePane), "status");
    }

    #[test]
    fn measured_reverse_f6_is_asymmetric_and_absent_surfaces_are_skipped() {
        let mut facts = facts();
        facts.share = true;
        facts.properties = true;
        for (at, next) in [
            ("list", "tree"),
            ("header", "list"),
            ("tree", "tab"),
            ("crumbs", "tab"),
            ("tab", "status"),
            ("status", "pane-share"),
            ("pane-properties", "tree"),
        ] {
            assert_eq!(destination(at, &facts, Step::ReversePane), next);
        }
        assert_eq!(
            destination("status", &facts, Step::ReverseTab),
            "pane-share"
        );
        facts.share = false;
        facts.properties = false;
        facts.upper = false;
        facts.tree = false;
        facts.header = false;
        facts.status = false;
        assert_eq!(destination("list", &facts, Step::Pane), "list");
        assert_eq!(destination("list", &facts, Step::ReverseTab), "list");
    }
}
