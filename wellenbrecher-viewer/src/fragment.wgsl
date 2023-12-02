@group(0) @binding(0) var canvas: texture_2d<f32>;
@group(0) @binding(1) var canvas_sampler: sampler;
@group(0) @binding(2) var user_id_map: texture_2d<u32>;

struct Push {
    blend_to: vec4<f32>,
    user_id_filter: u32,
    blending: f32,
}

var<push_constant> push: Push;

@fragment
fn main(@location(0) tex_coords: vec2<f32>) -> @location(0) vec4<f32> {
    var blending = 0.0;
    if (push.user_id_filter != u32(0)) {
        let user_id_map_coords = tex_coords * vec2<f32>(textureDimensions(canvas));
        if (push.user_id_filter != textureLoad(user_id_map, vec2<u32>(user_id_map_coords), 0).x) {
            blending = push.blending;
        }
    }
    return mix(textureSample(canvas, canvas_sampler, tex_coords), push.blend_to, blending);
}
