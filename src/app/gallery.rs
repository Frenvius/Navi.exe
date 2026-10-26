//! Virtualized Gallery mosaic, date range and scrubber. Windows owns collection locations.
use super::*;
use crate::shell::gallery::Photo;
use windows::Win32::Globalization::{GetDateFormatEx,DATE_YEARMONTH};
pub struct Gallery {
 pub viewport:(f32,f32),pub row_height:f32,pub icon_size:u32,pub offset:f32,pub extent:f32,pub rail_position:Option<f32>,
 pub cells:Vec<[f32;4]>,pub filter:Option<String>,pub filter_index:usize,
 pub(super) motion:Option<Motion>,
 pub retries:std::collections::HashMap<String,(u8,Instant)>,
}
#[derive(Clone,Copy)]pub(super) struct Motion{from:f32,to:f32,start:Instant}
impl Default for Gallery {fn default()->Self{Self{viewport:(598.0,559.0),row_height:160.0,icon_size:48,offset:0.0,extent:0.0,rail_position:None,cells:Vec::new(),filter:None,filter_index:0,motion:None,retries:Default::default()}}}
impl Gallery{pub fn set_size(&mut self,size:u32){self.icon_size=size;self.row_height=(size as f32*2.0+64.0).clamp(96.0,320.0);}}
fn zoom_size(size:u32,steps:i32)->u32{
 let mut height=(size as f32*2.0+64.0).clamp(96.0,320.0);
 for _ in 0..steps.unsigned_abs().min(64){let delta=(height*0.1/8.0).round().max(2.0)*8.0;height=(height+steps.signum() as f32*delta).clamp(96.0,320.0);}
 ((height-64.0)/2.0).round() as u32
}
fn aspect(photo:Photo)->f32{if photo.width==0||photo.height==0{1.0}else{(photo.width as f32/photo.height as f32).clamp(0.01,100.0)}}
pub fn mosaic(photos:&[Photo],width:f32,height:f32)->(Vec<[f32;4]>,f32){
 let mut cells=Vec::with_capacity(photos.len());let mut start=0;let mut y=0.0;
 while start<photos.len(){
  let mut end=start+1;let mut sum=aspect(photos[start]);
  let mut error=(width/sum-height).abs();
  while end<photos.len()&&end-start<32 {
   let next=sum+aspect(photos[end]);let avail=width-12.0*(end-start) as f32;
   if avail<=1.0{break;}
   let next_error=(avail/next-height).abs();
   if next_error>error{break;}
   end+=1;sum=next;error=next_error;
  }
  let available=(width-12.0*(end-start-1) as f32).max(1.0);let mut x:f32=32.0;
  for photo in &photos[start..end]{let w=available*aspect(*photo)/sum;cells.push([x.floor(),y,w.round().max(1.0),height]);x+=w+12.0;}
  y+=height+12.0;start=end;
 }
 (cells,if photos.is_empty(){0.0}else{y-12.0})
}
fn month(date:u64)->String{
 let Some(date)=group::local_time(date)else{return String::new()};let mut text=[0u16;128];
 let count=unsafe{GetDateFormatEx(None,DATE_YEARMONTH,Some(&date),None,Some(&mut text),None)};
 String::from_utf16_lossy(&text[..count.saturating_sub(1) as usize]).chars().filter(|c|!matches!(*c,'\u{200e}'|'\u{200f}')).collect()
}
impl App {
 pub fn gallery_viewport(&self,w:f32,h:f32){self.state.borrow_mut().gallery.viewport=(w,h);if ids::same(&self.current_path(),ids::GALLERY){self.layout_gallery();}}
 pub fn layout_gallery(&self){
  let (tiles,dates,heading,indicator,missing)={
   let mut st=self.state.borrow_mut();if !ids::same(&st.path,ids::GALLERY){return;}
   let photos:Vec<_>=st.order.iter().map(|&i|st.entries[i].photo.unwrap_or_default()).collect();
   let (w,h)=st.gallery.viewport;let (cells,extent)=mosaic(&photos,(w-120.0).max(64.0),st.gallery.row_height);
   st.gallery.cells=cells;st.gallery.extent=extent;
   let page=(h-61.0).max(1.0);st.gallery.offset=st.gallery.offset.clamp(0.0,(extent-page).max(0.0));
   let offset=st.gallery.offset;let mut tiles=Vec::new();let mut missing=Vec::new();let images=self.images.borrow();
   for (row,&i) in st.order.iter().enumerate(){
    let [x,y,width,height]=st.gallery.cells[row];if y+height<offset-page||y>offset+page*2.0{continue;}
    let e=&st.entries[i];let picture=images.thumb(&e.path,768).unwrap_or_default();if picture.size().width==0 {missing.push(e.path.clone());}
    tiles.push(crate::GalleryTile{row:row as i32,x,y:y-offset,width,height,picture,selected:st.selected.contains(&e.path),name:e.name.as_str().into()});
   }
   let mut dates=Vec::new();let mut last_year=0;let mut years=Vec::new();
   for (row,photo) in photos.iter().enumerate(){
    let year=group::local_time(photo.date).map_or(0,|t|t.wYear);if year==0||year==last_year{continue;}last_year=year;
    let y=75.0+st.gallery.cells[row][1]/extent.max(1.0)*(h-116.0).max(1.0);
    years.push((year,y));
   }
   for (index,&(year,y)) in years.iter().enumerate(){let end=years.get(index+1).map_or(h-41.0,|v|v.1);if end-y>=12.0{dates.push(crate::GalleryDate{label:year.to_string().into(),y:y.round()});}}
   let first=st.gallery.cells.iter().position(|c|c[1]+c[3]>offset).unwrap_or(0);
   let last=st.gallery.cells.iter().rposition(|c|c[1]<offset+page).unwrap_or(first);
   let a=photos.get(first).map_or(String::new(),|p|month(p.date));let b=photos.get(last).map_or(String::new(),|p|month(p.date));
   let heading=if a==b||b.is_empty(){a}else{format!("{a} - {b}")};
   let indicator=st.gallery.rail_position.unwrap_or(80.0+offset/extent.max(1.0)*(h-116.0).max(1.0));
   (tiles,dates,heading,indicator,missing)
  };
  let ui=self.ui();let shell=ui.global::<Shell>();
  update_model(&self.gallery_tiles,tiles);update_model(&self.gallery_dates,dates);
  shell.set_gallery_heading(heading.into());shell.set_gallery_indicator(indicator);
  shell.set_gallery_collection(crate::i18n::system_string("Windows.UI.FileExplorer.dll",51856).into());
  shell.set_gallery_collection_icon(self.images.borrow_mut().svg("windows.galleryfiltercommand"));
  for path in missing{self.request_thumb(&path,768);}
 }
 pub fn gallery_scroll(self:&Rc<Self>,delta:f32){
  {let mut st=self.state.borrow_mut();st.gallery.rail_position=None;let max=(st.gallery.extent-(st.gallery.viewport.1-61.0)).max(0.0);let to=(st.gallery.motion.map_or(st.gallery.offset,|m|m.to)+delta).clamp(0.0,max);st.gallery.motion=Some(Motion{from:st.gallery.offset,to,start:Instant::now()});}
  let weak=Rc::downgrade(self);
  self.gallery_timer.start(slint::TimerMode::Repeated,std::time::Duration::from_millis(16),move||{
   let Some(app)=weak.upgrade()else{return};
   let done={let mut st=app.state.borrow_mut();if !ids::same(&st.path,ids::GALLERY){st.gallery.motion=None;}
    if let Some(motion)=st.gallery.motion{let t=((motion.start.elapsed().as_secs_f32()-0.032)/0.250).clamp(0.0,1.0);let progress=1.0-(1.0-t)*(1.0-t);st.gallery.offset=motion.from+(motion.to-motion.from)*progress;if t>=1.0{st.gallery.motion=None;}t>=1.0}else{true}};
   if done{app.gallery_timer.stop();}app.layout_gallery();
  });
 }
 pub fn gallery_scrub(&self,y:f32,initial:bool){
  self.gallery_timer.stop();
  {let mut st=self.state.borrow_mut();let h=st.gallery.viewport.1;let extent=st.gallery.extent;st.gallery.motion=None;
   let indicator=80.0+st.gallery.offset/extent.max(1.0)*(h-116.0).max(1.0);
   let year=if initial&&(y-indicator).abs()>12.0 {self.ui().global::<Shell>().get_gallery_dates().iter().find(|date|y>=date.y&&y<date.y+19.0).and_then(|date|date.label.parse::<u16>().ok())}else{None};
   let row=year.and_then(|year|st.order.iter().position(|&i|group::local_time(st.entries[i].photo.unwrap_or_default().date).is_some_and(|d|d.wYear==year)));
   st.gallery.offset=if let Some(row)=row{st.gallery.cells[row][1]}else{((y-80.0)/(h-116.0).max(1.0)).clamp(0.0,1.0)*extent};st.gallery.rail_position=Some(y);}
  self.layout_gallery();
 }
 pub fn gallery_date_at(&self,y:f32)->SharedString{
  let st=self.state.borrow();let h=st.gallery.viewport.1;
  let offset=((y-80.0)/(h-116.0).max(1.0)).clamp(0.0,1.0)*st.gallery.extent;
  let row=st.gallery.cells.iter().position(|c|c[1]+c[3]>offset).unwrap_or(st.order.len().saturating_sub(1));
  st.order.get(row).map(|&i|month(st.entries[i].photo.unwrap_or_default().date)).unwrap_or_default().into()
 }
 pub fn gallery_zoom(&self,steps:i32){
  let size=zoom_size(self.state.borrow().gallery.icon_size,steps);self.gallery_set_size(size);
 }
 pub fn gallery_set_size(&self,size:u32){
  {let mut st=self.state.borrow_mut();let old=st.gallery.row_height;st.view_dirty|=st.gallery.icon_size!=size;st.gallery.set_size(size);st.gallery.offset*=st.gallery.row_height/old;}
  self.layout_gallery();self.sync_view_state();
 }
 pub fn gallery_filter(self:&Rc<Self>,index:usize){
  self.gallery_timer.stop();
  let root=if index==0{None}else{crate::shell::gallery::collection_roots().get(index-1).cloned()};
  {let mut st=self.state.borrow_mut();st.gallery.filter=root;st.gallery.filter_index=index;st.gallery.offset=0.0;st.gallery.rail_position=None;st.gallery.motion=None;}
  self.rebuild_order();self.sync_rows();self.sync_view_state();
 }
}
fn update_model<T:Clone+PartialEq+'static>(model:&VecModel<T>,rows:Vec<T>){
 for(index,row)in rows.iter().enumerate(){if index>=model.row_count(){model.push(row.clone());}else if model.row_data(index).as_ref()!=Some(row){model.set_row_data(index,row.clone());}}
 while model.row_count()>rows.len(){model.remove(model.row_count()-1);}
}
#[cfg(test)]mod tests{
 use super::*;
 #[test]fn native_gallery_zoom_steps(){for(size,direction,expected)in[(16,1,24),(16,-1,16),(48,1,56),(48,-1,40),(96,1,108),(96,-1,84)]{assert_eq!(zoom_size(size,direction),expected);}}
 #[test]fn native_mosaic_rows_normalize_aspects_and_keep_160px_crop(){
  let dims=[(1693,982),(478,72),(269,238),(759,484),(1004,631),(354,226),(589,240)];
  let photos:Vec<_>=dims.into_iter().map(|(width,height)|Photo{width,height,date:0}).collect();
  let (cells,extent)=mosaic(&photos,478.0,160.0);
  assert_eq!(cells[0],[32.0,0.0,96.0,160.0]);assert_eq!(cells[1],[140.0,0.0,370.0,160.0]);
  assert_eq!(cells[2],[32.0,172.0,195.0,160.0]);assert_eq!(cells[3],[239.0,172.0,271.0,160.0]);
  assert_eq!(cells[4],[32.0,344.0,235.0,160.0]);assert_eq!(cells[5],[278.0,344.0,231.0,160.0]);
  assert_eq!(cells[6],[32.0,516.0,478.0,160.0]);assert_eq!(extent,676.0);
 }
}
