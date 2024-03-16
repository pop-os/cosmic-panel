// Author: Ashley Wulber
// Title: Rounded rectangle
#extension GL_OES_standard_derivatives : enable
#ifdef GL_ES
precision mediump float;
#endif

// canvas
uniform vec2 size;

uniform float rad_tl;
uniform float rad_tr;
uniform float rad_bl;
uniform float rad_br;
uniform vec2 loc;
uniform vec2 rect_size;

vec4 corner(float rad, vec2 corner, vec2 point, vec3 c) {
    if (rad <= 0.) {
        return vec4(c, 1.);
    }
    vec2 diff = abs(corner - point);
    float mh_dist = diff.x + diff.y;
    float distance_to_corner = length(diff);
    // for anti-aliasing
    float delta = fwidth(distance_to_corner);
    if (distance_to_corner <= rad - delta / 2. || mh_dist <= rad) {
        return vec4(c, 1.);
    }
    
    return vec4(c, 1. - smoothstep(rad - delta / 2., rad + delta / 2., distance_to_corner));
}

void main() {
    vec2 tl_corner = vec2(loc.x + rad_tl, loc.y + rect_size.y - rad_tl);
    vec2 tr_corner = vec2(loc.x + rect_size.x - rad_tr, loc.y + rect_size.y - rad_tr);
    vec2 bl_corner = vec2(loc.x + rad_bl, loc.y + rad_bl);
    vec2 br_corner = vec2(loc.x + rect_size.x - rad_br, loc.y + rad_br);

    vec2 p = gl_FragCoord.xy;

    vec3 color = vec3(1.,0.,1.0);
    
    if (p.x < loc.x || p.x > loc.x + rect_size.x || p.y < loc.y || p.y > loc.y + rect_size.y) {
        gl_FragColor = vec4(0.);
    } else if (p.x < tl_corner.x && p.y > tl_corner.y) {
        gl_FragColor = corner(rad_tl, tl_corner, p, color);
    } else if (p.x > tr_corner.x && p.y > tr_corner.y) {
        gl_FragColor = corner(rad_tr, tr_corner, p, color);
    } else if (p.x < bl_corner.x && p.y < bl_corner.y) {
        gl_FragColor = corner(rad_bl, bl_corner, p, color);
    } else if (p.x > br_corner.x && p.y < br_corner.y) {
        gl_FragColor = corner(rad_bl, br_corner, p, color);
    } else {
        gl_FragColor = vec4(color,1.0);
    }
}

