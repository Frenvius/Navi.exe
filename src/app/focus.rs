//! Explorer's Tab ring and its distinct F6 pane traversal.
use super::*;

pub const ZONES: &[&str] = &[
    "list",
    "header",
    "status",
    "tab",
    "new-tab",
    "back",
    "forward",
    "up",
    "refresh",
    "address",
    "crumbs",
    "search",
    "commands",
    "pane-command",
    "tree",
    "pane-share",
    "pane-properties",
    "neutral",
    "settings",
    "settings-section",
    "settings-toggle",
    "settings-restore",
];

impl App {
    pub fn command_ids(&self) -> Vec<&'static str> {
        let ui = self.ui();
        let s = ui.global::<Shell>();
        let mut ids = Vec::new();
        if s.get_can_new() {
            ids.push("new");
        }
        for (id, enabled) in [
            ("cut", s.get_can_cut()),
            ("copy", s.get_can_copy()),
            ("paste", s.get_can_paste()),
            ("rename", s.get_can_rename()),
            ("share", s.get_can_share()),
            ("delete", s.get_can_delete()),
        ] {
            if enabled {
                ids.push(id);
            }
        }
        if !s.get_home_view() && !s.get_gallery_view() {
            ids.push("sort");
        }
        ids.push("view");
        if s.get_gallery_view(){ids.push("collection");}
        if s.get_home_view() {
            ids.push("filter");
        }
        if s.get_search_view() {
            ids.push("search-options");
            if s.get_close_search_in_bar() {
                ids.push("close-search");
            }
        }
        if s.get_recycle_view() && s.get_recycle_has_items() {
            ids.push("empty-bin");
        }
        if s.get_extract_view() {
            ids.push("extract-all");
        }
        if s.get_wallpaper() {
            ids.push("wallpaper");
        }
        ids.extend(["more", "details-pane"]);
        ids
    }

    pub fn focus_command(&self, last: bool) {
        let commands = self.command_ids();
        let id = if last { "more" } else { commands[0] };
        let ui = self.ui();
        let s = ui.global::<Shell>();
        s.set_command_focus(id.into());
        s.set_command_focus_request(s.get_command_focus_request() + 1);
    }

    pub fn note_focus(&self, zone: &str) {
        self.ui().global::<Shell>().set_focus_zone(zone.into());
        if std::env::var_os("FOCUS_TRACE").is_some() {
            crate::frame::focus_debug(
                self.hwnd.get(),
                ZONES.iter().position(|&z| z == zone).unwrap_or(0),
            );
        }
    }

    pub fn step_focus(&self, reverse: bool, fast: bool) {
        let ui = self.ui();
        let shell = ui.global::<Shell>();
        let pane = ui.global::<crate::Pane>();
        let at = shell.get_focus_zone();
        let at = at.as_str();
        if shell.get_settings_view() {
            let mut ring=vec!["tab","new-tab"];
            if shell.get_can_back(){ring.push("back");} if shell.get_can_forward(){ring.push("forward");}
            ring.extend(["refresh","address","crumbs","settings","settings-section","settings-toggle"]);
            if shell.get_default_recovery() && !shell.get_default_enabled(){ring.push("settings-restore");}
            let at=ring.iter().position(|z|*z==at).unwrap_or(ring.len()-1) as i32;
            let next=(at+if reverse{-1}else{1}).rem_euclid(ring.len() as i32) as usize;
            ui.invoke_focus_target(ring[next].into());return;
        }
        if shell.get_native_shell_view(){
            let mut order=if fast{vec!["list","crumbs","search"]}else{vec!["back","forward","up","crumbs","search","settings","list"]};
            order.retain(|zone|match *zone{"back"=>shell.get_can_back(),"forward"=>shell.get_can_forward(),"up"=>shell.get_can_up(),_=>true});
            let at=order.iter().position(|zone|*zone==at).unwrap_or(0) as i32;
            let next=(at+if reverse{-1}else{1}).rem_euclid(order.len() as i32) as usize;
            ui.invoke_focus_target(order[next].into());return;
        }
        let header = !shell.get_icons_view() && !shell.get_drives_view() && !shell.get_home_view();
        let tree = shell.get_sidebar_width() > 0.0;
        let upper = !shell.get_full_screen();
        let share = shell.get_details_pane() && pane.get_can_share();
        let props = shell.get_details_pane() && pane.get_can_properties();
        let first_pane = if share {
            Some("pane-share")
        } else if props {
            Some("pane-properties")
        } else {
            None
        };
        if at == "neutral" {
            ui.invoke_focus_target(if reverse {
                if shell.get_status_bar() {"status"}else{first_pane.unwrap_or(if header {"header"}else{"list"})}.into()
            } else if tree {
                "tree".into()
            } else {
                "list".into()
            });
            return;
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
                if shell.get_can_back() {
                    ring.push("back");
                }
                if shell.get_can_forward() {
                    ring.push("forward");
                }
                if shell.get_can_up() {
                    ring.push("up");
                }
                ring.extend([
                    "refresh",
                    "address",
                    "crumbs",
                    "search",
                    "settings",
                    "commands",
                    "pane-command",
                ]);
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
            if shell.get_status_bar() {ring.push("status");}
            let i = ring
                .iter()
                .position(|&z| z == at)
                .unwrap_or_else(|| ring.iter().position(|&z| z == "list").unwrap());
            // Entering the details pane backwards goes to its first button, even when
            // the forward traversal has both Share and Properties.
            if reverse && at == "status" && first_pane.is_some() {
                first_pane.unwrap()
            } else {
                ring[(i as isize + if reverse { -1 } else { 1 }).rem_euclid(ring.len() as isize)
                    as usize]
            }
        };
        let destination=if destination=="status" && !shell.get_status_bar() {
            if reverse {first_pane.unwrap_or(if header {"header"}else{"list"})}
            else if upper {"tab"}else if tree {"tree"}else{"list"}
        }else{destination};
        ui.invoke_focus_target(if destination == "commands" && reverse && !fast {
            "commands-last".into()
        } else {
            destination.into()
        });
    }
}
