//! Opcode tables for revision 274. Length -1 means one plain length
//! byte follows the opcode, -2 means two plain length bytes, anything
//! else is a fixed payload size. Opcode bytes on the wire are ISAAC-shifted.

/// (client op id, fixed payload size or -1/-2 for framed).
pub fn client_op(name: &str) -> Option<(u8, i16)> {
    Some(match name {
        "NO_TIMEOUT" => (120, 0),
        "IDLE_TIMER" => (209, 0),
        "EVENT_MOUSE_CLICK" => (20, 4),
        "EVENT_MOUSE_MOVE" => (222, -1),
        "EVENT_APPLET_FOCUS" => (73, 1),
        "EVENT_CAMERA_POSITION" => (53, 4),
        "OPOBJ1" => (247, 6),
        "OPOBJ2" => (169, 6),
        "OPOBJ3" => (108, 6),
        "OPOBJ4" => (62, 6),
        "OPOBJ5" => (117, 6),
        "OPOBJT" => (91, 8),
        "OPOBJU" => (39, 12),
        "OPNPC1" => (236, 2),
        "OPNPC2" => (233, 2),
        "OPNPC3" => (223, 2),
        "OPNPC4" => (147, 2),
        "OPNPC5" => (189, 2),
        "OPNPCT" => (181, 4),
        "OPNPCU" => (150, 8),
        "OPLOC1" => (215, 6),
        "OPLOC2" => (103, 6),
        "OPLOC3" => (187, 6),
        "OPLOC4" => (157, 6),
        "OPLOC5" => (127, 6),
        "OPLOCT" => (213, 8),
        "OPLOCU" => (60, 12),
        "OPPLAYER1" => (109, 2),
        "OPPLAYER2" => (166, 2),
        "OPPLAYER3" => (196, 2),
        "OPPLAYER4" => (98, 2),
        "OPPLAYER5" => (174, 2),
        "OPPLAYERT" => (240, 4),
        "OPPLAYERU" => (36, 8),
        "OPHELD1" => (185, 6),
        "OPHELD2" => (2, 6),
        "OPHELD3" => (123, 6),
        "OPHELD4" => (216, 6),
        "OPHELD5" => (42, 6),
        "OPHELDT" => (135, 8),
        "OPHELDU" => (136, 12),
        "INV_BUTTON1" => (74, 6),
        "INV_BUTTON2" => (82, 6),
        "INV_BUTTON3" => (239, 6),
        "INV_BUTTON4" => (179, 6),
        "INV_BUTTON5" => (46, 6),
        "IF_BUTTON" => (9, 2),
        "RESUME_PAUSEBUTTON" => (72, 2),
        "CLOSE_MODAL" => (51, 0),
        "RESUME_P_COUNTDIALOG" => (102, 4),
        "TUT_CLICKSIDE" => (94, 1),
        "MAP_BUILD_COMPLETE" => (214, 0),
        "MOVE_OPCLICK" => (138, -1),
        "MOVE_MINIMAPCLICK" => (86, -1),
        "MOVE_GAMECLICK" => (207, -1),
        "INV_BUTTOND" => (93, 7),
        "IGNORELIST_DEL" => (101, 8),
        "IGNORELIST_ADD" => (255, 8),
        "IDK_SAVEDESIGN" => (125, 13),
        "CHAT_SETMODE" => (154, 3),
        "MESSAGE_PRIVATE" => (139, -1),
        "FRIENDLIST_DEL" => (106, 8),
        "FRIENDLIST_ADD" => (13, 8),
        "CLIENT_CHEAT" => (224, -1),
        "MESSAGE_PUBLIC" => (253, -1),
        _ => return None,
    })
}

/// (server op name, fixed payload size or -1/-2 for framed).
pub fn server_op(op_id: u8) -> Option<(&'static str, i16)> {
    Some(match op_id {
        166 => ("IF_OPENCHAT", 2),
        158 => ("IF_OPENMAIN_SIDE", 4),
        171 => ("IF_CLOSE", 0),
        215 => ("IF_SETTAB", 3),
        241 => ("IF_SETTAB_ACTIVE", 1),
        211 => ("IF_OPENMAIN", 2),
        16 => ("IF_OPENSIDE", 2),
        240 => ("IF_OPENOVERLAY", 2),
        183 => ("IF_SETCOLOUR", 4),
        10 => ("IF_SETHIDE", 3),
        28 => ("IF_SETOBJECT", 6),
        129 => ("IF_SETMODEL", 4),
        134 => ("IF_SETANIM", 4),
        192 => ("IF_SETPLAYERHEAD", 2),
        44 => ("IF_SETTEXT", -2),
        142 => ("IF_SETNPCHEAD", 4),
        77 => ("IF_SETPOSITION", 6),
        54 => ("IF_SETSCROLLPOS", 4),
        90 => ("TUT_FLASH", 1),
        130 => ("TUT_OPEN", 2),
        227 => ("UPDATE_INV_STOP_TRANSMIT", 2),
        106 => ("UPDATE_INV_FULL", -2),
        172 => ("UPDATE_INV_PARTIAL", -2),
        233 => ("CAM_LOOKAT", 6),
        64 => ("CAM_SHAKE", 4),
        200 => ("CAM_MOVETO", 6),
        101 => ("CAM_RESET", 0),
        197 => ("NPC_INFO", -2),
        167 => ("PLAYER_INFO", -2),
        185 => ("FRIENDLIST_LOADED", 1),
        161 => ("MESSAGE_GAME", -1),
        3 => ("UPDATE_IGNORELIST", -2),
        114 => ("CHAT_FILTER_SETTINGS", 3),
        235 => ("MESSAGE_PRIVATE", -1),
        247 => ("UPDATE_FRIENDLIST", 9),
        115 => ("UNSET_MAP_FLAG", 0),
        67 => ("UPDATE_RUNWEIGHT", 2),
        156 => ("HINT_ARROW", 6),
        89 => ("UPDATE_REBOOT_TIMER", 2),
        105 => ("UPDATE_STAT", 6),
        83 => ("UPDATE_RUNENERGY", 1),
        47 => ("RESET_ANIMS", 0),
        133 => ("UPDATE_PID", 3),
        91 => ("LAST_LOGIN_INFO", 10),
        88 => ("LOGOUT", 0),
        210 => ("P_COUNTDIALOG", 0),
        207 => ("SET_MULTIWAY", 1),
        17 => ("SET_PLAYER_OP", -1),
        194 => ("MINIMAP_TOGGLE", 1),
        231 => ("REBUILD_NORMAL", 4),
        203 => ("VARP_SMALL", 3),
        245 => ("VARP_LARGE", 6),
        190 => ("RESET_CLIENT_VARCACHE", 0),
        34 => ("SYNTH_SOUND", 5),
        23 => ("MIDI_SONG", 2),
        15 => ("MIDI_JINGLE", 4),
        32 => ("UPDATE_ZONE_PARTIAL_FOLLOWS", 2),
        153 => ("UPDATE_ZONE_FULL_FOLLOWS", 2),
        195 => ("UPDATE_ZONE_PARTIAL_ENCLOSED", -2),
        81 => ("OBJ_ADD", 5),
        52 => ("OBJ_DEL", 3),
        95 => ("OBJ_COUNT", 7),
        219 => ("OBJ_REVEAL", 7),
        138 => ("LOC_ADD_CHANGE", 4),
        173 => ("LOC_DEL", 2),
        48 => ("LOC_ANIM", 4),
        176 => ("LOC_MERGE", 14),
        85 => ("MAP_ANIM", 6),
        107 => ("MAP_PROJANIM", 15),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::{client_op, server_op};

    #[test]
    fn known_ops_resolve() {
        assert_eq!(client_op("MOVE_GAMECLICK"), Some((207, -1)));
        assert_eq!(client_op("NO_TIMEOUT"), Some((120, 0)));
        assert_eq!(client_op("OPNPCT"), Some((181, 4)));
        assert_eq!(client_op("OPOBJT"), Some((91, 8)), "telegrab brick");
        assert_eq!(client_op("OPHELDT"), Some((135, 8)), "alch/superheat/enchant brick");
        assert_eq!(client_op("OPLOCT"), Some((213, 8)), "scenery spell brick");
        assert_eq!(server_op(167), Some(("PLAYER_INFO", -2)));
        assert_eq!(server_op(81), Some(("OBJ_ADD", 5)));
        assert_eq!(client_op("NOPE"), None);
    }
}
