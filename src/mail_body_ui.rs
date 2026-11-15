//! 邮件正文的块渲染：岛内详情卡用它把 Doc 画成带样式、可点链接的正文。
//!
//! 「阅读全文」窗口用上游自己的 `mail_reader::body_ui`（纯文本行 + 轻排版），
//! 这里只服务岛内那张小卡片，两边互不影响。
use crate::mail_doc::{Align,Block,Doc,Paragraph,Run,Table};
use crate::mail_view::{self,Tokens,ViewMode};
use eframe::egui::{self,Color32,Stroke};

/// 一次渲染的模式与令牌，沿调用链往下传，避免每帧重算。
#[derive(Clone,Copy)]
pub(crate) struct RenderStyle {
    pub mode:ViewMode,
    pub tokens:Tokens,
}
impl RenderStyle {
    pub fn new(mode:ViewMode)->Self{Self{mode,tokens:mail_view::tokens(mode)}}
}

/// 邮件多为白底深字，岛仔是深色界面。直接照搬颜色会看不清，
/// 这里把偏暗的文字按亮度往浅色提，保留色相（蓝色链接还是蓝的）。
fn readable(color:Color32)->Color32{
    let luma=0.299*color.r() as f32+0.587*color.g() as f32+0.114*color.b() as f32;
    // 邮件正文常用 #57606a 这类中灰，原先只提到 118 亮度，在 #151518 上仍然发闷。
    // 统一往白色提到至少 150，色相按比例保留（蓝还是蓝、红还是红）。
    const MIN_LUMA:f32=150.0;
    if luma>=MIN_LUMA{return color;}
    let k=(MIN_LUMA-luma)/(255.0-luma).max(1.0);
    let lift=|channel:u8|(channel as f32+(255.0-channel as f32)*k).min(255.0) as u8;
    Color32::from_rgba_unmultiplied(lift(color.r()),lift(color.g()),lift(color.b()),color.a())
}

/// 识别「整段就是一个短链接」的 CTA：返回链接地址、按钮文字与可选底色。
/// 没有底色的独立链接也当成按钮（Outlined 样式），比一行居中下划线文字清楚得多。
fn cta(para:&Paragraph)->Option<(std::sync::Arc<str>,String,Option<Color32>)>{
    let text:String=para.runs.iter().map(|run|run.text.as_str()).collect();
    let label=text.trim().to_string();
    if label.is_empty()||label.chars().count()>28||label.contains('\n'){return None;}
    let mut url:Option<std::sync::Arc<str>>=None;
    for run in &para.runs{
        if run.text.trim().is_empty(){continue;}
        let Some(link)=&run.link else{return None;};
        if url.as_deref().is_some_and(|u|u!=link.as_ref()){return None;}
        url=Some(link.clone());
    }
    Some((url?,label,para.bg))
}

/// 排版结果：LayoutJob 加上每段链接在字符流里的范围。
struct Prepared{
    job:egui::text::LayoutJob,
    links:Vec<(std::ops::Range<usize>,std::sync::Arc<str>)>,
}

/// 把一组带样式的片段塞进一个 LayoutJob，并记录链接的字符范围。
fn prepare_job(runs:&[Run],scale:f32,width:f32,align:Align,style:RenderStyle)->Prepared{
    let mut job=egui::text::LayoutJob::default();
    job.wrap.max_width=width.max(40.0);
    job.halign=match align{Align::Center=>egui::Align::Center,Align::Right=>egui::Align::Max,_=>egui::Align::Min};
    let mut links=Vec::new();
    let mut index=0usize;
    for run in runs{
        for (part_index,part) in run.text.split('\n').enumerate(){
            if part_index>0{
                job.append("\n",0.0,egui::text::TextFormat::default());
                index+=1;
            }
            if part.is_empty(){continue;}
            let raw_size=if style.tokens.uniform_size{style.tokens.body_size}else{run.size};
            let size=(raw_size*scale).max(9.0);
            // 标题行收紧行距，正文行距按模式令牌走；极简模式统一字色，但链接仍是品牌蓝。
            let heading=raw_size>=17.0||(run.bold&&raw_size>=16.0);
            let line_ratio=if heading{style.tokens.line_heading}else{style.tokens.line_body};
            let color=if run.link.is_some()&&(run.color==Color32::from_gray(232)||style.tokens.uniform_color){
                crate::theme::BLUE
            }else if style.tokens.uniform_color{
                Color32::from_gray(224)
            }else{
                readable(run.color)
            };
            if let Some(link)=&run.link{
                let chars=part.chars().count();
                links.push((index..index+chars,link.clone()));
                index+=chars;
            }else{
                index+=part.chars().count();
            }
            job.append(part,0.0,egui::text::TextFormat{
                font_id:if run.mono{
                    egui::FontId::monospace(size*0.96)
                }else{
                    crate::theme::font(size,if run.bold{crate::theme::W::Bold}else{crate::theme::W::Regular})
                },
                color,
                italics:run.italic,
                underline:if run.underline{Stroke::new(1.0,color)}else{Stroke::NONE},
                line_height:Some((size*line_ratio).round()),
                ..Default::default()
            });
        }
    }
    Prepared{job,links}
}

/// 渲染一段可能带内联链接的文字：返回 (被点击的地址, 绘制区域)。
fn rich_label(ui:&mut egui::Ui,runs:&[Run],scale:f32,width:f32,align:Align,selectable:bool,style:RenderStyle)->(Option<String>,egui::Rect){
    let prepared=prepare_job(runs,scale,width,align,style);
    let galley=ui.fonts(|fonts|fonts.layout_job(prepared.job));
    let has_links=!prepared.links.is_empty();
    let mut label=egui::Label::new(galley.clone());
    if has_links{
        label=label.sense(egui::Sense::click());
    }else if selectable{
        label=label.selectable(true);
    }
    let response=ui.add(label);
    let mut clicked=None;
    if has_links{
        let hit=|pos:egui::Pos2|->Option<String>{
            let cursor=galley.cursor_from_pos(pos-response.rect.min);
            let index=cursor.ccursor.index;
            prepared.links.iter().find(|(range,_)|range.contains(&index)).map(|(_,url)|url.to_string())
        };
        if response.hovered(){
            if let Some(pos)=ui.ctx().pointer_hover_pos(){
                if hit(pos).is_some(){ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);}
            }
        }
        if response.clicked(){
            if let Some(pos)=response.interact_pointer_pos(){clicked=hit(pos);}
        }
    }
    (clicked,response.rect)
}

/// CTA：邮件里「整格只有一个短链接」的按钮，还原成 Filled Tonal 胶囊。
/// 有强调底色就用邮件自己的品牌色（GitHub 绿、NVIDIA 绿），没有则用细边框的文字按钮。
fn cta_button(ui:&mut egui::Ui,label:&str,url:&std::sync::Arc<str>,bg:Option<Color32>,align:Align,scale:f32)->Option<String>{
    let (fill,ink,stroke)=match bg{
        Some(color)=>(color,Color32::WHITE,Stroke::NONE),
        None=>(Color32::from_rgba_unmultiplied(90,150,220,24),crate::theme::BLUE,
               Stroke::new(1.0,Color32::from_rgba_unmultiplied(90,150,220,96))),
    };
    let button=egui::Button::new(
        egui::RichText::new(label).size((13.0*scale).max(11.0)).strong().color(ink))
        .fill(fill).stroke(stroke).corner_radius(9.0)
        .min_size(egui::vec2(0.0,32.0*scale.max(0.9)));
    let mut clicked=None;
    let draw=|ui:&mut egui::Ui|{
        let response=ui.add(button).on_hover_cursor(egui::CursorIcon::PointingHand);
        if response.clicked(){Some(url.to_string())}else{None}
    };
    match align{
        Align::Center=>{ui.vertical_centered(|ui|{clicked=draw(ui);});}
        Align::Right=>{ui.with_layout(egui::Layout::right_to_left(egui::Align::Min),|ui|{clicked=draw(ui);});}
        Align::Left=>{clicked=draw(ui);}
    }
    ui.add_space(6.0*scale);
    clicked
}

/// 带源邮件背景色的段落：铺一层淡底卡片再画文字。
fn paragraph_ui(ui:&mut egui::Ui,para:&Paragraph,scale:f32,marker:Option<&str>,style:RenderStyle)->Option<String>{
    if marker.is_none(){
        if let Some((url,label,bg))=cta(para){
            return cta_button(ui,&label,&url,bg,para.align,scale);
        }
    }
    if let Some(bg)=para.bg.filter(|_|style.tokens.decoration){
        let luma=0.299*bg.r() as f32+0.587*bg.g() as f32+0.114*bg.b() as f32;
        let alpha=if luma>=200.0{34}else{52};
        let mut clicked=None;
        egui::Frame::new()
            .fill(Color32::from_rgba_unmultiplied(bg.r(),bg.g(),bg.b(),alpha))
            .corner_radius(7.0)
            .inner_margin(egui::Margin::symmetric(10,7))
            .show(ui,|ui|{clicked=paragraph_body(ui,para,scale,marker,style);});
        return clicked;
    }
    paragraph_body(ui,para,scale,marker,style)
}

fn paragraph_body(ui:&mut egui::Ui,para:&Paragraph,scale:f32,marker:Option<&str>,style:RenderStyle)->Option<String>{
    if para.before>0.0{ui.add_space((para.before+style.tokens.gap_level*0.25)*scale);}
    let row=ui.available_width();
    let indent=para.indent*scale;
    let width=(row-indent-2.0).max(60.0);
    let mut clicked=None;
    // 有缩进或标记时按行排；纯居中/右对齐的段落交给布局推过去。
    let drawn=if marker.is_some()||indent>0.0{
        let mut rect=egui::Rect::NOTHING;
        ui.horizontal(|ui|{
            ui.spacing_mut().item_spacing.x=0.0;
            if indent>0.0{ui.add_space(indent);}
            if let Some(marker)=marker{
                ui.add(egui::Label::new(egui::RichText::new(marker).size(11.0*scale.max(0.9)).color(Color32::from_gray(190))).wrap());
            }
            let (url,area)=rich_label(ui,&para.runs,scale,width,para.align,true,style);
            if url.is_some(){clicked=url;}
            rect=area;
        });
        rect
    }else{
        let layout=match para.align{
            Align::Center=>egui::Layout::top_down(egui::Align::Center),
            Align::Right=>egui::Layout::top_down(egui::Align::Max),
            Align::Left=>egui::Layout::top_down(egui::Align::Min),
        };
        let mut rect=egui::Rect::NOTHING;
        ui.with_layout(layout,|ui|{
            let (url,area)=rich_label(ui,&para.runs,scale,width,para.align,true,style);
            if url.is_some(){clicked=url;}
            rect=area;
        });
        rect
    };
    if para.quote{
        // 引用用左侧细线标记，和邮件客户端的观感一致。
        ui.painter().vline(drawn.left()+2.0,drawn.y_range(),Stroke::new(3.0,Color32::from_rgb(70,110,150)));
    }
    if para.after>0.0{ui.add_space((para.after+style.tokens.gap_same*0.15)*scale);}
    clicked
}

fn cell_label(ui:&mut egui::Ui,runs:&[Run],scale:f32,width:f32,align:Align,style:RenderStyle)->Option<String>{
    match align{
        Align::Center=>{
            let mut clicked=None;
            ui.vertical_centered(|ui|{
                let (url,_)=rich_label(ui,runs,scale,width,align,true,style);
                if url.is_some(){clicked=url;}
            });
            clicked
        }
        Align::Right=>{
            let mut clicked=None;
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min),|ui|{
                let (url,_)=rich_label(ui,runs,scale,width,align,true,style);
                if url.is_some(){clicked=url;}
            });
            clicked
        }
        Align::Left=>rich_label(ui,runs,scale,width,align,true,style).0,
    }
}

fn table_ui(ui:&mut egui::Ui,table:&Table,scale:f32,style:RenderStyle,images:&crate::mail_image::ImageCache,embedded:&std::collections::HashMap<String,std::sync::Arc<Vec<u8>>>)->Option<String>{
    // 走到这里的基本是有边框的数据表格，列数不该太多；单元格至少留 120px，
    // 否则中文会被压成一行两三个字。
    let columns=table.rows.iter().map(|r|r.cells.iter().map(|c|c.span).sum::<usize>()).max().unwrap_or(1).clamp(1,6);
    let mut clicked=None;
    let stroke=if style.tokens.decoration{Stroke::new(1.0,Color32::from_gray(66))}else{Stroke::NONE};
    egui::Frame::new()
        .stroke(stroke)
        .corner_radius(10.0)
        .inner_margin(egui::Margin::symmetric(10,8))
        .show(ui,|ui|{
            // 固定列宽：居中/右对齐的单元格原本会把 Grid 给的一整行宽度占满，
            // 底色就变成一条贯穿整行的灰条。
            let total=ui.available_width();
            let cell_width=((total-6.0*columns as f32)/columns as f32).max(120.0);
            let job_width=(cell_width-16.0).max(60.0);
            egui::Grid::new(("mail-table",ui.next_auto_id())).num_columns(columns).spacing([6.0,6.0]).show(ui,|ui|{
                for row in &table.rows{
                    for cell in &row.cells{
                        // 空单元格只占位、不画框。布局表里的空 td 以前会被画成一条条灰横条，
                        // 把正文压在下面，是「一排排灰色矩形」的主要来源。
                        let has_text=cell.runs.iter().any(|run|!run.text.trim().is_empty());
                        if !has_text&&cell.images.is_empty(){
                            for _ in 0..cell.span.max(1){ui.label("");}
                            continue;
                        }
                        // 只有邮件明确给了强调色才铺底；表头/斑马纹一律不铺，
                        // 深色界面上中性灰底只会变成脏块。
                        let fill=cell.bg
                            .and_then(crate::mail_doc::visible_bg)
                            .filter(|_|style.tokens.decoration)
                            .map(|bg|Color32::from_rgba_unmultiplied(bg.r(),bg.g(),bg.b(),46))
                            .unwrap_or(Color32::TRANSPARENT);
                        egui::Frame::new().fill(fill).corner_radius(6.0)
                            .inner_margin(egui::Margin::symmetric(8,5))
                            .show(ui,|ui|{
                                ui.set_width(job_width);
                                if let Some(url)=cell_label(ui,&cell.runs,scale,job_width,cell.align,style){clicked=Some(url);}
                                // 单元格里的图片（logo / hero / 商品图）在文字之后排。
                                for image in &cell.images{
                                    images.show(ui,image,job_width,scale,embedded);
                                }
                            });
                        for _ in 1..cell.span{ui.label("");}
                    }
                    ui.end_row();
                }
            });
        });
    clicked
}

/// 正文按块渲染：保留邮件自己的字号、字重、颜色与对齐。
/// 返回正文内被点击的链接地址。
pub(crate) fn body_ui(ui:&mut egui::Ui,doc:&Doc,base:f32,images:&crate::mail_image::ImageCache,embedded:&std::collections::HashMap<String,std::sync::Arc<Vec<u8>>>,style:RenderStyle)->Option<String>{
    // 邮件正文按 15px 设计；卡片和阅读窗口各自缩放，避免大字号把窄卡片撑爆。
    let scale=(base/15.0).clamp(0.72,1.15);
    ui.spacing_mut().item_spacing.y=match style.mode{
        ViewMode::Fidelity=>3.0,ViewMode::Comfort=>5.0,ViewMode::Minimal=>4.0,
    };
    let mut clicked=None;
    for block in &doc.blocks{
        let hit=match block{
            Block::Paragraph(para)=>paragraph_ui(ui,para,scale,None,style),
            Block::ListItem(item)=>paragraph_ui(ui,&item.para,scale,Some(&item.marker),style),
            Block::Image(image)=>{
                ui.add_space(style.tokens.image_gap*scale*0.5);
                images.show(ui,image,ui.available_width(),scale,embedded);
                ui.add_space(style.tokens.image_gap*scale*0.5);
                None
            }
            Block::Table(table)=>table_ui(ui,table,scale,style,images,embedded),
            Block::Divider=>{
                let gap=style.tokens.gap_block*scale*0.4;
                ui.add_space(gap);
                ui.separator();
                ui.add_space(gap);
                None
            }
            Block::Space(height)=>{ui.add_space(height*scale);None}
        };
        if hit.is_some(){clicked=hit;}
    }
    clicked
}
