@group(0) @binding(0) var canvas: texture_2d<f32>;
@group(0) @binding(1) var canvas_sampler: sampler;
@group(0) @binding(2) var user_id_map: texture_storage_2d<r32uint, read>;
@group(0) @binding(3) var secondary_canvas: texture_storage_2d<rgba8unorm, read_write>;
@group(0) @binding(4) var secondary_user_id_map: texture_storage_2d<r32uint, read_write>;
@group(0) @binding(5) var<storage, read_write> state: FragmentShaderState;

struct Push {
    blend_to: vec4<f32>,
    user_id_filter: u32,
    blending: f32,
}

struct FragmentShaderState {
    last_highlighted_uid: u32
}

var<push_constant> push: Push;

@fragment
fn main(@location(0) tex_coords: vec2<f32>) -> @location(0) vec4<f32> {
    let primary_canvas_color = textureSample(canvas, canvas_sampler, tex_coords);
    var color = primary_canvas_color;
    var blending = 0.0;

    if (push.user_id_filter != u32(0)) {
        let coords = vec2<u32>(tex_coords * vec2<f32>(textureDimensions(canvas)));

        // clear secondary canvas, when highlighting changes
        if (state.last_highlighted_uid != push.user_id_filter) {
            textureStore(secondary_canvas, coords, vec4<f32>(0.0));
            textureStore(secondary_user_id_map, coords, vec4<u32>(u32(0)));
        }

        // append to secondary canvas
        let primary_canvas_uid = textureLoad(user_id_map, coords).x;
        if (push.user_id_filter == primary_canvas_uid) {
            textureStore(secondary_canvas, coords, primary_canvas_color);
            textureStore(secondary_user_id_map, coords, vec4<u32>(primary_canvas_uid));
        }

        // use color from secondary pixel, if uid matches the highlighting
        let secondary_canvas_uid = textureLoad(secondary_user_id_map, coords).x;
        if (secondary_canvas_uid == push.user_id_filter) {
            color = textureLoad(secondary_canvas, coords);
        } else {
            blending = push.blending;
        }
    }

    state.last_highlighted_uid = push.user_id_filter;
    return mix(color, push.blend_to, blending);
}
