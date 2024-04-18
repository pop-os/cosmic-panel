// Author: Ashley Wulber
// Title: Rounded rectangle
#extension GL_OES_standard_derivatives:enable
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

uniform float border_width;
uniform float drop_shadow;
uniform vec4 bg_color;
uniform vec4 border_color;

float sdRoundBox(in vec2 p,in vec2 b,in vec4 r)
{
    r.xy=(p.x>0.)?r.xy:r.zw;
    r.x=(p.y>0.)?r.x:r.y;
    vec2 q=abs(p)-b+r.x;
    return min(max(q.x,q.y),0.)+length(max(q,0.))-r.x;
}

void main()
{
    vec2 p=2.*gl_FragCoord.xy-(rect_size+loc*2.);
    
    vec2 si=rect_size;
    vec4 ra=2.*vec4(rad_tr,rad_br,rad_tl,rad_bl);
    ra=min(ra,min(si.x,si.y));
    
    float d=sdRoundBox(p,si,ra);
    
    vec2 tl_corner=vec2(loc.x,loc.y+rect_size.y);
    vec2 tr_corner=vec2(loc.x+rect_size.x,loc.y+rect_size.y);
    vec2 bl_corner=vec2(loc.x,loc.y);
    vec2 br_corner=vec2(loc.x+rect_size.x,loc.y);
    vec2 pos=gl_FragCoord.xy;
    
    float delta=0.;
    float d_tl;
    float d_tr;
    float d_bl;
    float d_br;
    if(dot(tl_corner,tr_corner)>dot(tl_corner,bl_corner)){
        d_tl=abs(tl_corner.x-pos.x);
        d_tr=abs(tr_corner.x-pos.x);
        d_bl=abs(bl_corner.x-pos.x);
        d_br=abs(br_corner.x-pos.x);
    }else{
        d_tl=abs(tl_corner.y-pos.y);
        d_tr=abs(tr_corner.y-pos.y);
        d_bl=abs(bl_corner.y-pos.y);
        d_br=abs(br_corner.y-pos.y);
    }
    vec4 calc_bg_color;
    
    if(d_tl<=rad_tl){
        delta=(ra.z-d_tl)/rad_tl*fwidth(d)/2.;
    }else if(d_tr<=rad_tr){
        delta=(ra.x-d_tr)/rad_tr*fwidth(d)/2.;
    }else if(d_bl<=rad_bl){
        delta=(ra.w-d_bl)/rad_bl*fwidth(d)/2.;
    }else if(d_br<=rad_br){
        delta=(ra.y-d_br)/rad_br*fwidth(d)/2.;
    }
    
    float a=1.-smoothstep(1.-5.*delta/6.,1.+delta/6.,1.+d);
    
    gl_FragColor=vec4(0.,0.,0.,a);
}

