//! 正射投影地球。底图来自 Natural Earth 公有领域数据，完全离线。
use crate::{paint::{Card,H,mix},theme::{self,W}};
use chrono::{DateTime,Datelike,Timelike,Utc};
use eframe::egui::{self,Color32,Pos2,Rect,TextureHandle};
use std::{collections::HashMap,f32::consts::PI};

const LAND: &[u8;65536] = include_bytes!("../../assets/world-clock/land-mask.bin");

pub(super) fn coordinates(zone:&str)->Option<(f32,f32)> {
    Some(match zone {
        "Asia/Shanghai"=>(116.4,39.9), "Asia/Hong_Kong"=>(114.2,22.3),
        "Asia/Taipei"=>(121.6,25.0), "Asia/Tokyo"=>(139.7,35.7),
        "Asia/Seoul"=>(127.0,37.6), "Asia/Singapore"=>(103.8,1.4),
        "Asia/Bangkok"=>(100.5,13.8), "Asia/Kolkata"=>(77.2,28.6),
        "Asia/Kathmandu"=>(85.3,27.7), "Asia/Dubai"=>(55.3,25.2),
        "Europe/London"=>(-0.1,51.5), "Europe/Paris"=>(2.4,48.9),
        "Europe/Berlin"=>(13.4,52.5), "Europe/Moscow"=>(37.6,55.8),
        "America/New_York"=>(-74.0,40.7), "America/Chicago"=>(-87.6,41.9),
        "America/Denver"=>(-105.0,39.7), "America/Los_Angeles"=>(-118.2,34.1),
        "America/Vancouver"=>(-123.1,49.3), "America/Toronto"=>(-79.4,43.7),
        "America/Sao_Paulo"=>(-46.6,-23.6), "Australia/Sydney"=>(151.2,-33.9),
        "Australia/Adelaide"=>(138.6,-34.9), "Pacific/Auckland"=>(174.8,-36.8),
        "Africa/Cairo"=>(31.2,30.0), "Africa/Johannesburg"=>(28.0,-26.2),
        _=>{
            static CITIES:std::sync::OnceLock<HashMap<String,(f32,f32)>>=std::sync::OnceLock::new();
            return CITIES.get_or_init(||serde_json::from_str(include_str!("../../assets/world-clock/cities.json")).expect("bundled IANA timezone coordinates")).get(zone).copied();
        },
    })
}

struct Texture {key:(i32,i32,i64),handle:TextureHandle}
pub(super) struct Globe {pub longitude:f32,pub latitude:f32,textures:HashMap<usize,Texture>}
impl Default for Globe {
    fn default()->Self {Self{longitude:105.0,latitude:25.0,textures:HashMap::new()}}
}

fn normal(lon:f32,lat:f32)->[f32;3] {
    let(lon,lat)=(lon.to_radians(),lat.to_radians());
    [lat.cos()*lon.cos(),lat.sin(),lat.cos()*lon.sin()]
}
fn dot(a:[f32;3],b:[f32;3])->f32 {a[0]*b[0]+a[1]*b[1]+a[2]*b[2]}
fn basis(lon:f32,lat:f32)->[[f32;3];3] {
    let(lon,lat)=(lon.to_radians(),lat.to_radians());
    [[-lon.sin(),0.0,lon.cos()],[-lat.sin()*lon.cos(),lat.cos(),-lat.sin()*lon.sin()],[lat.cos()*lon.cos(),lat.sin(),lat.cos()*lon.sin()]]
}
fn project(point:[f32;3],basis:[[f32;3];3],center:Pos2,r:f32)->Option<Pos2> {
    (dot(point,basis[2])>0.025).then(||center+egui::vec2(dot(point,basis[0])*r,-dot(point,basis[1])*r))
}

impl Globe {
    pub fn center_on(&mut self,zone:&str) {
        if let Some((lon,lat))=coordinates(zone){self.longitude=lon;self.latitude=lat;}
    }

    pub fn draw(&mut self,c:&Card,area:Rect,now:DateTime<Utc>,cities:&[String],selected:usize,labels:bool)->Option<usize> {
        let center=area.center();let radius=area.width().min(area.height())/2.0;
        let response=c.hit_drag(area.left(),area.top(),area.width(),area.height(),("world-globe",labels))
            .on_hover_cursor(egui::CursorIcon::Grab).on_hover_text("拖动旋转地球，点击城市定位");
        if response.dragged() {
            let delta=c.ui.ctx().input(|i|i.pointer.delta())/c.s;
            self.longitude=(self.longitude-delta.x*0.65).rem_euclid(360.0);
            self.latitude=(self.latitude+delta.y*0.5).clamp(-75.0,75.0);
            c.ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }
        let pixels=(radius*2.0*c.s*c.ui.ctx().pixels_per_point()).round().clamp(128.0,512.0) as usize;
        let key=((self.longitude*2.0).round()as i32,(self.latitude*2.0).round()as i32,now.timestamp()/60);
        if !self.textures.get(&pixels).is_some_and(|cached|cached.key==key) {
            let image=earth(pixels,self.longitude,self.latitude,now);
            if !self.textures.contains_key(&pixels) && self.textures.len()>=2 {self.textures.clear();}
            if let Some(cached)=self.textures.get_mut(&pixels){cached.handle.set(image,egui::TextureOptions::LINEAR);cached.key=key;}
            else {self.textures.insert(pixels,Texture{key,handle:c.ui.ctx().load_texture(format!("world-earth-{pixels}"),image,egui::TextureOptions::LINEAR)});}
        }
        c.painter.image(self.textures[&pixels].handle.id(),c.rect(area.left(),area.top(),area.width(),area.height()),Rect::from_min_max(Pos2::ZERO,Pos2::new(1.0,1.0)),Color32::WHITE);
        c.ring(center.x,center.y,radius,0.7,theme::BLUE.gamma_multiply(0.45));
        let camera=basis(self.longitude,self.latitude);
        let line=|points:Vec<[f32;3]>| {
            let mut segment=Vec::new();
            for point in points {
                if let Some(point)=project(point,camera,center,radius){segment.push(c.pos(point.x,point.y));}
                else if !segment.is_empty(){c.painter.add(egui::Shape::line(std::mem::take(&mut segment),egui::Stroke::new(0.6*c.s,Color32::from_white_alpha(22))));}
            }
            if !segment.is_empty(){c.painter.add(egui::Shape::line(segment,egui::Stroke::new(0.6*c.s,Color32::from_white_alpha(22))));}
        };
        for lat in [-60,-30,0,30,60] {line((-180..=180).step_by(4).map(|lon|normal(lon as f32,lat as f32)).collect());}
        for lon in (-180..180).step_by(30) {line((-90..=90).step_by(3).map(|lat|normal(lon as f32,lat as f32)).collect());}
        let mut clicked=None;let mut occupied:Vec<Rect>=Vec::new();
        let order=(0..cities.len()).filter(|index|*index!=selected).chain(std::iter::once(selected));
        for index in order {
            let Some(zone)=cities.get(index) else{continue;};
            let Some((lon,lat))=coordinates(zone) else{continue;};
            let Some(p)=project(normal(lon,lat),camera,center,radius) else{continue;};
            let active=index==selected;let color=if active{theme::YELLOW}else{theme::CYAN};
            if active {c.circle(p.x,p.y,6.0,color.gamma_multiply(0.2));c.ring(p.x,p.y,4.0,1.0,color);}
            c.circle(p.x,p.y,if active{2.2}else{1.8},color);
            if labels || active {
                let name=super::city_name(zone);let size=if active{11.0}else{9.5};
                let width=c.text_w(&name,size,W::Semibold);let left=p.x<center.x;
                let x=if left{p.x+8.0}else{p.x-8.0};let h=if left{H::Left}else{H::Right};
                let rect=Rect::from_min_size(Pos2::new(if left{x}else{x-width},p.y-7.0),egui::vec2(width,14.0));
                if active || !occupied.iter().any(|r|r.intersects(rect)) {
                    c.fill(rect.left()-3.0,rect.top()-1.0,rect.width()+6.0,16.0,4.0,theme::CARD.gamma_multiply(0.88));
                    c.centered_text(x,p.y,&name,size,W::Semibold,if active{theme::WHITE}else{theme::GRAY},h);occupied.push(rect);
                }
            }
            if c.hit(p.x-8.0,p.y-8.0,16.0,16.0,("world-globe-city",labels,index)).on_hover_text(super::city_name(zone)).clicked(){clicked=Some(index);}
        }
        clicked
    }
}

fn earth(size:usize,lon:f32,lat:f32,now:DateTime<Utc>)->egui::ColorImage {
    let camera=basis(lon,lat);
    // 太阳赤纬的低精度近似，用于视觉昼夜示意，不用作日出日落预报。
    let declination=23.44*(2.0*PI*(now.ordinal()as f32-81.0)/365.2422).sin();
    let sun=normal(180.0-(now.hour()as f32+now.minute()as f32/60.0)*15.0,declination);
    let day_land=mix(theme::CYAN,theme::WHITE,0.78);
    let night_land=mix(theme::BLUE_DEEP,theme::GRAY,0.30);
    let day_water=mix(theme::CARD,theme::BLUE_DEEP,0.45);
    let night_water=mix(theme::CARD,Color32::BLACK,0.65);
    let mut image=egui::ColorImage::new([size,size],Color32::TRANSPARENT);
    for y in 0..size {for x in 0..size {
        let sx=(x as f32+0.5)/size as f32*2.0-1.0;let sy=1.0-(y as f32+0.5)/size as f32*2.0;
        let square=sx*sx+sy*sy;if square>=1.0{continue;}
        let sz=(1.0-square).sqrt();
        let point=std::array::from_fn(|i|sx*camera[0][i]+sy*camera[1][i]+sz*camera[2][i]);
        let lon=point[2].atan2(point[0]);let lat=point[1].clamp(-1.0,1.0).asin();
        let mx=(((lon+PI)/(2.0*PI)*1024.0)as usize).min(1023);
        let my=(((PI/2.0-lat)/PI*512.0)as usize).min(511);let bit=my*1024+mx;
        let land=LAND[bit/8]&(1<<(bit%8))!=0;
        let light=crate::anim::smoothstep(-0.12,0.16,dot(point,sun));
        let color=if land{mix(night_land,day_land,light)}else{mix(night_water,day_water,light)};
        let edge=((1.0-square)*size as f32/2.0).clamp(0.0,1.0);
        image.pixels[y*size+x]=color.gamma_multiply((0.65+0.35*sz.sqrt())*edge);
    }}
    image
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn orthographic_projection_hides_the_back_of_the_globe() {
        let camera=basis(0.0,0.0);let center=Pos2::new(50.0,50.0);
        assert_eq!(project(normal(0.0,0.0),camera,center,40.0),Some(center));
        assert!(project(normal(180.0,0.0),camera,center,40.0).is_none());
        assert!(project(normal(45.0,0.0),camera,center,40.0).unwrap().x>center.x);
    }
    #[test]
    fn land_mask_distinguishes_continents_from_ocean() {
        let land=|lon:f32,lat:f32|{let i=((90.0-lat)/180.0*512.0)as usize*1024+((lon+180.0)/360.0*1024.0)as usize;LAND[i/8]&(1<<(i%8))!=0};
        assert!(land(100.0,35.0));assert!(land(-100.0,40.0));assert!(!land(-140.0,0.0));
        let rome=coordinates("Europe/Rome").unwrap();assert!((rome.0-12.48).abs()<0.1 && (rome.1-41.9).abs()<0.1);
        let now=DateTime::parse_from_rfc3339("2026-06-21T12:00:00Z").unwrap().with_timezone(&Utc);
        let image=earth(128,0.0,0.0,now);assert_eq!(image.pixels[0],Color32::TRANSPARENT);assert!(image.pixels[64*128+64].a()>0);
    }
}
