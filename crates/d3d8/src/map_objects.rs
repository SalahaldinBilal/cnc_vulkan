//! The objects placed in the map file, for the radar's map drawn from above. The map file's
//! object list is the same for a new game and a loaded save, so this shows the map as it was
//! made, whatever was built or destroyed since. Each object gets a drawable the way startNewGame
//! makes one for a tree, whose models the scene draws with its lights, and is destroyed after the
//! draw. See knowledge-base/radar.md.

use rustc_hash::FxHashSet;

use crate::game;

struct Addresses {
    the_map_object_list: usize,
    /// MapObject::getThingTemplate, thiscall.
    get_thing_template: usize,
    the_thing_factory: usize,
    /// ThingFactory::newDrawable(template, status bits), thiscall.
    new_drawable: usize,
    /// Thing::setOrientation(angle) and Drawable::setPosition(position), thiscall.
    set_orientation: usize,
    set_position: usize,
    the_game_client: usize,
    the_terrain_logic: usize,
    the_game_logic: usize,
    /// The recorder, and whether it plays a multiplayer game, which turns trees on.
    the_recorder: usize,
    recorder_is_multiplayer: usize,
    /// What startNewGame reads to decide whether clutter becomes a prop. Generals has no props.
    fluff_rules: Option<FluffRules>,
    /// ThingTemplate: whether it's a bridge.
    is_bridge: usize,
    /// Bridges become objects in Zero Hour. Generals leaves them to the terrain.
    bridges_are_objects: bool,
    bridge_kind: (usize, u32),
    condition_flags: usize,
    model_dirty: usize,
    draw_modules: usize,
    replace_condition_state_vslot: usize,
    /// The game holds model changes back while this is above 0.
    model_change_lock: usize,
    rts3d_scene_vtable: usize,
    /// RenderObjClass Get_User_Data, a DrawableInfo for a drawable's models, the drawable at +4.
    get_user_data_vslot: usize,
    /// RTS3DScene::renderOneObject(render info, render object, local player index), thiscall.
    render_one_object: usize,
    the_player_list: usize,
    /// DoTrees(render info), cdecl: the terrain's tree buffer.
    do_trees: usize,
}

struct FluffRules {
    game_mode_owner: usize,
    game_mode: usize,
    /// GlobalData: keep clutter in replays.
    keep_fluff: usize,
}

static ZERO_HOUR: Addresses = Addresses {
    the_map_object_list: 0x00A4_88D0,
    get_thing_template: 0x0074_3880,
    the_thing_factory: 0x00A3_9B24,
    new_drawable: 0x004A_DC00,
    set_orientation: 0x0054_42B0,
    set_position: 0x0047_6700,
    the_game_client: 0x00A3_9B18,
    the_terrain_logic: 0x00A3_9594,
    the_game_logic: 0x00A3_9B0C,
    the_recorder: 0x00A3_9914,
    recorder_is_multiplayer: 0x0048_E3A0,
    fluff_rules: Some(FluffRules { game_mode_owner: 0x00A3_9844, game_mode: 0xA00, keep_fluff: 0x52 }),
    is_bridge: 0x228,
    bridges_are_objects: true,
    bridge_kind: (1, 0x80_0000),
    condition_flags: 0x118,
    model_dirty: 0x14C,
    draw_modules: 0xC0,
    replace_condition_state_vslot: 0x2C / 4,
    model_change_lock: 0x00A3_9840,
    rts3d_scene_vtable: 0x0096_87B4,
    get_user_data_vslot: 0x150 / 4,
    render_one_object: 0x0076_6D30,
    the_player_list: 0x00A3_95A0,
    do_trees: 0x0074_FC10,
};

static GENERALS: Addresses = Addresses {
    the_map_object_list: 0x00A7_2988,
    get_thing_template: 0x0071_CFE0,
    the_thing_factory: 0x00A6_BE9C,
    new_drawable: 0x004A_E4B0,
    set_orientation: 0x0056_00F0,
    set_position: 0x0047_4940,
    the_game_client: 0x00A6_BE90,
    the_terrain_logic: 0x00A6_B958,
    the_game_logic: 0x00A6_BE84,
    the_recorder: 0x00A6_BC88,
    recorder_is_multiplayer: 0x0048_9EA0,
    fluff_rules: None,
    is_bridge: 0x23B,
    bridges_are_objects: false,
    bridge_kind: (1, 0x100_0000),
    condition_flags: 0x11C,
    model_dirty: 0x149,
    draw_modules: 0xC4,
    replace_condition_state_vslot: 0x24 / 4,
    model_change_lock: 0x00A6_BBF0,
    rts3d_scene_vtable: 0x009A_63C0,
    get_user_data_vslot: 0x154 / 4,
    render_one_object: 0x0074_65C0,
    the_player_list: 0x00A6_B968,
    do_trees: 0x0072_87B0,
};

fn addresses() -> &'static Addresses {
    game::pick(&ZERO_HOUR, Some(&GENERALS)).unwrap_or(&ZERO_HOUR)
}

mod map_object {
    pub const LOCATION: usize = 0x4;
    pub const ANGLE: usize = 0x18;
    pub const NEXT: usize = 0x1C;
    pub const FLAGS: usize = 0x20;
    /// Road and bridge points, which the terrain draws.
    pub const ROAD_AND_BRIDGE_POINTS: u32 = 0x36;
}

mod template {
    /// KindOf bits, four words.
    pub const KIND_OF: usize = 0x68;
    pub const FENCE_WIDTH: usize = 0x1CC;
}

/// Zero Hour's KindOf bits, as (word, bit). Shrubbery is the same in Generals.
mod kind_of {
    pub const SHRUBBERY: (usize, u32) = (0, 0x40);
    pub const CLEARED_BY_BUILD: (usize, u32) = (1, 0x4000);
    pub const PROP: (usize, u32) = (3, 0x20);
    pub const OPTIMIZED_TREE: (usize, u32) = (3, 0x40);
}

const DESTROY_DRAWABLE_VSLOT: usize = 0x4C / 4;
/// TerrainLogic::getGroundHeight(x, y, normal), thiscall.
const GROUND_HEIGHT_VSLOT: usize = 0x14 / 4;

mod global_data {
    pub const USE_TREES: usize = 0x10;
    pub const TIME_OF_DAY: usize = 0x204;
    pub const NIGHT: i32 = 4;
    pub const MAKE_TRACK_MARKS: usize = 0x20C;
}

/// GameLogic: set while a map loads.
const LOADING_MAP: usize = 0x51;

mod drawable {
    pub const NIGHT_FLAG: u32 = 0x80;
    pub const OBJECT_DRAW_INTERFACE_VSLOT: usize = 0x50 / 4;
}

/// SimpleSceneClass's render list: a list head, nodes with the next one at +4 and the object's
/// list link at +0xc, which sits 0x10 into the render object.
const RENDER_LIST: usize = 0x3C;
const NODE_NEXT: usize = 0x4;
const NODE_OBJECT: usize = 0xC;
const LIST_LINK: usize = 0x10;
const DRAWABLE_INFO_DRAWABLE: usize = 0x4;
const LOCAL_PLAYER: usize = 0xC;
const PLAYER_INDEX: usize = 0x24;

fn read<T: Copy>(at: usize) -> T {
    unsafe { *(at as *const T) }
}

fn has(template: usize, (word, bit): (usize, u32)) -> bool {
    read::<u32>(template + template::KIND_OF + word * 4) & bit != 0
}

unsafe fn vcall(object: usize, slot: usize) -> usize {
    unsafe { *read::<*const usize>(object).add(slot) }
}

pub fn loading_map() -> bool {
    let logic = read::<usize>(addresses().the_game_logic);
    logic == 0 || read::<u8>(logic + LOADING_MAP) != 0
}

/// startNewGame's two switches: shrubbery only with trees on, and in Zero Hour clutter turned into
/// props unless the game mode keeps it, both forced on for a multiplayer recording.
fn tree_and_prop_rules(a: &Addresses, gd: usize) -> (bool, bool) {
    let mut use_trees = read::<u8>(gd + global_data::USE_TREES) != 0;
    let mut fluff_to_prop = a.fluff_rules.as_ref().is_some_and(|f| {
        let mode = read::<i32>(read::<usize>(f.game_mode_owner) + f.game_mode);
        mode != 2 && !(mode == 3 && read::<u8>(gd + f.keep_fluff) != 0)
    });
    let recorder = read::<usize>(a.the_recorder);
    if recorder != 0 {
        let is_multiplayer: extern "thiscall" fn(usize) -> u8 = unsafe { std::mem::transmute(a.recorder_is_multiplayer) };
        if is_multiplayer(recorder) != 0 {
            use_trees = true;
            fluff_to_prop = a.fluff_rules.is_some();
        }
    }
    (use_trees, fluff_to_prop)
}

/// Whether startNewGame makes an object from this entry, rather than leaving it to the terrain,
/// the tree buffer or the prop buffer.
fn becomes_object(a: &Addresses, template: usize, use_trees: bool, fluff_to_prop: bool) -> bool {
    if has(template, a.bridge_kind) || read::<u8>(template + a.is_bridge) != 0 {
        return a.bridges_are_objects;
    }
    if has(template, kind_of::SHRUBBERY) && !use_trees {
        return false;
    }
    if a.fluff_rules.is_none() {
        return true;
    }
    if has(template, kind_of::OPTIMIZED_TREE) || has(template, kind_of::PROP) {
        return false;
    }
    let fluff = has(template, kind_of::CLEARED_BY_BUILD) && read::<f32>(template + template::FENCE_WIDTH) == 0.0;
    !(fluff && fluff_to_prop)
}

/// A drawable for each object in the map file, placed and turned as the map has it.
pub unsafe fn create() -> Vec<usize> {
    let a = addresses();
    let Some(gd) = game::build().map(|b| read::<usize>(b.global_data)).filter(|&gd| gd != 0) else { return Vec::new() };
    let (factory, client, terrain) = (read::<usize>(a.the_thing_factory), read::<usize>(a.the_game_client), read::<usize>(a.the_terrain_logic));
    let logic = read::<usize>(a.the_game_logic);
    if factory == 0 || client == 0 || terrain == 0 || logic == 0 {
        return Vec::new();
    }
    let (use_trees, fluff_to_prop) = tree_and_prop_rules(a, gd);
    let night = read::<i32>(gd + global_data::TIME_OF_DAY) == global_data::NIGHT;
    let mut drawables = Vec::new();
    unsafe {
        let get_template: extern "thiscall" fn(usize) -> usize = std::mem::transmute(a.get_thing_template);
        let new_drawable: extern "thiscall" fn(usize, usize, u32) -> usize = std::mem::transmute(a.new_drawable);
        let set_orientation: extern "thiscall" fn(usize, f32) = std::mem::transmute(a.set_orientation);
        let set_position: extern "thiscall" fn(usize, *const [f32; 3]) = std::mem::transmute(a.set_position);
        let ground_height: extern "thiscall" fn(usize, f32, f32, usize) -> f32 = std::mem::transmute(vcall(terrain, GROUND_HEIGHT_VSLOT));
        // A drawable made while a map loads doesn't start its ambient sound.
        let loading = (logic + LOADING_MAP) as *mut u8;
        let was_loading = *loading;
        *loading = 1;
        // A model with track marks updates them on every move by reading the drawable's Object,
        // which ours don't have.
        let track_marks = (gd + global_data::MAKE_TRACK_MARKS) as *mut u8;
        let made_track_marks = *track_marks;
        *track_marks = 0;
        let mut entry = read::<usize>(a.the_map_object_list);
        while entry != 0 {
            let next = read::<usize>(entry + map_object::NEXT);
            let template = if read::<u32>(entry + map_object::FLAGS) & map_object::ROAD_AND_BRIDGE_POINTS == 0 { get_template(entry) } else { 0 };
            if template != 0 && becomes_object(a, template, use_trees, fluff_to_prop) {
                let draw = new_drawable(factory, template, 0);
                if draw != 0 {
                    let mut position = read::<[f32; 3]>(entry + map_object::LOCATION);
                    position[2] += ground_height(terrain, position[0], position[1], 0);
                    set_orientation(draw, read::<f32>(entry + map_object::ANGLE));
                    set_position(draw, &position);
                    if night {
                        set_night(a, draw);
                    }
                    drawables.push(draw);
                }
            }
            entry = next;
        }
        *track_marks = made_track_marks;
        *loading = was_loading;
    }
    drawables
}

/// Drawable::setTimeOfDay's night flag, without its sound, applied to the models right away the
/// way the game applies a changed model state.
unsafe fn set_night(a: &Addresses, draw: usize) {
    if read::<i32>(a.model_change_lock) > 0 {
        return;
    }
    unsafe {
        *((draw + a.condition_flags) as *mut u32) |= drawable::NIGHT_FLAG;
        let mut module = read::<usize>(draw + a.draw_modules) as *const usize;
        while *module != 0 {
            let get_interface: extern "thiscall" fn(usize) -> usize = std::mem::transmute(vcall(*module, drawable::OBJECT_DRAW_INTERFACE_VSLOT));
            let interface = get_interface(*module);
            if interface != 0 {
                let replace: extern "thiscall" fn(usize, usize) = std::mem::transmute(vcall(interface, a.replace_condition_state_vslot));
                replace(interface, draw + a.condition_flags);
            }
            module = module.add(1);
        }
        *((draw + a.model_dirty) as *mut u8) = 0;
    }
}

/// Draws the models of the given drawables through the scene, which lights each one. Returns how
/// many models were drawn.
pub unsafe fn render(scene: usize, render_info: *mut u8, drawables: &[usize]) -> usize {
    let a = addresses();
    if drawables.is_empty() || read::<usize>(scene) != a.rts3d_scene_vtable {
        return 0;
    }
    let ours: FxHashSet<usize> = drawables.iter().copied().collect();
    let players = read::<usize>(a.the_player_list);
    let local_player = if players == 0 { 0 } else { read::<i32>(read::<usize>(players + LOCAL_PLAYER) + PLAYER_INDEX) };
    let mut drawn = 0;
    unsafe {
        let render_one: extern "thiscall" fn(usize, *mut u8, usize, i32) = std::mem::transmute(a.render_one_object);
        let head = scene + RENDER_LIST;
        let mut node = read::<usize>(head + NODE_NEXT);
        while node != head && node != 0 {
            let link = read::<usize>(node + NODE_OBJECT);
            node = read::<usize>(node + NODE_NEXT);
            if link == 0 {
                continue;
            }
            let object = link - LIST_LINK;
            let get_user_data: extern "thiscall" fn(usize) -> usize = std::mem::transmute(vcall(object, a.get_user_data_vslot));
            let info = get_user_data(object);
            if info != 0 && ours.contains(&read::<usize>(info + DRAWABLE_INFO_DRAWABLE)) {
                render_one(scene, render_info, object, local_player);
                drawn += 1;
            }
        }
    }
    drawn
}

pub unsafe fn destroy(drawables: &[usize]) {
    let client = read::<usize>(addresses().the_game_client);
    if client == 0 {
        return;
    }
    unsafe {
        let destroy: extern "thiscall" fn(usize, usize) = std::mem::transmute(vcall(client, DESTROY_DRAWABLE_VSLOT));
        for &draw in drawables {
            destroy(client, draw);
        }
    }
}

pub unsafe fn draw_trees(render_info: *mut u8) {
    unsafe {
        let do_trees: extern "C" fn(*mut u8) = std::mem::transmute(addresses().do_trees);
        do_trees(render_info);
    }
}
