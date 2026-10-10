//! Externally generated 3D `.cube` LUTs and catalog indexing.
//! No internal LUT rebaking. LUT source files are immutable and loaded only on selection.
use std::{collections::HashMap,fs,path::{Path,PathBuf}};
use serde::{Deserialize,Serialize};

#[derive(Debug,Clone,Copy,PartialEq,Eq,Serialize,Deserialize)]
#[serde(rename_all="snake_case")]
pub enum InputSpace { Srgb, Rec2020LinearSignedLog }
impl InputSpace {
    pub fn name(self)->&'static str {match self{Self::Srgb=>"sRGB (encoded)",Self::Rec2020LinearSignedLog=>"Linear Rec.2020 / signed-log"}}
}
#[derive(Debug,Clone)]
pub struct Cube {pub size:u32, pub data:Vec<[f32;4]>,pub min:[f32;3],pub max:[f32;3],pub title:String}
#[derive(Debug,Clone)]
pub struct LutEntry {pub path:PathBuf,pub label:String,pub film:String,pub paper:String,pub stage:String,pub input:InputSpace}

pub fn parse_cube(bytes:&str)->Result<Cube,String>{
    let mut size=None;let mut title=String::new();let mut min=[0.0f32;3];let mut max=[1.0f32;3];let mut data=Vec::new();
    for (line_no,line) in bytes.lines().enumerate(){
        let line=line.split('#').next().unwrap_or("").trim();if line.is_empty(){continue}
        let tokens:Vec<&str>=line.split_whitespace().collect();
        match tokens[0].to_ascii_uppercase().as_str(){
            "TITLE"=>{title=line[5..].trim().trim_matches('"').to_string();}
            "LUT_3D_SIZE"=>{let n:usize=tokens.get(1).ok_or("Missing LUT_3D_SIZE")?.parse().map_err(|_|"Invalid LUT size")?;
                if !(2..=129).contains(&n){return Err("3D LUT size outside 2..129".into())}size=Some(n);}
            "LUT_1D_SIZE"=>return Err("1D and 1D+3D LUT chains need a separate shaper contract; refusing to misinterpret them".into()),
            "DOMAIN_MIN"|"DOMAIN_MAX"=>{if tokens.len()!=4{return Err(format!("Line {}: invalid domain",line_no+1))};let mut v=[0.;3];for i in 0..3{v[i]=tokens[i+1].parse().map_err(|_|"Invalid domain number")?;}if tokens[0].eq_ignore_ascii_case("DOMAIN_MIN"){min=v}else{max=v}},
            _=>{if tokens.len()!=3{return Err(format!("Line {}: unsupported LUT directive",line_no+1))};if size.is_none(){return Err("LUT data precedes LUT_3D_SIZE".into())};
                let mut v=[0f32;4];for i in 0..3{v[i]=tokens[i].parse().map_err(|_|format!("Line {}: invalid sample",line_no+1))?;if !v[i].is_finite(){return Err("Non-finite LUT value".into());}}v[3]=1.;data.push(v);}
        }
    }
    let n=size.ok_or("Missing LUT_3D_SIZE")?;
    if data.len()!=n*n*n {return Err(format!("Expected {} RGB points, got {}",n*n*n,data.len()))}
    for i in 0..3{if !min[i].is_finite()||!max[i].is_finite()||max[i]<=min[i]{return Err("Invalid LUT domain".into())}}
    Ok(Cube{size:n as u32,data,min,max,title})
}

#[derive(Debug,Default,Deserialize)]
struct Contract {input_space:Option<String>,output_space:Option<String>,shaper:Option<String>,stage:Option<String>}
fn contract_for(path:&Path)->Result<Option<Contract>,String>{
    let json=path.with_extension("json"); if !json.is_file(){return Ok(None)}
    let file=fs::read_to_string(&json).map_err(|e|e.to_string())?;
    let c:Contract=serde_json::from_str(&file).map_err(|e|format!("{}: {e}",json.display()))?;
    Ok(Some(c))
}
fn contract_input(c:Option<&Contract>,csv_input:&str)->Result<InputSpace,String>{
    let raw=c.and_then(|v|v.input_space.as_deref()).unwrap_or(csv_input).trim().to_ascii_lowercase();
    let input=match raw.as_str(){
        ""|"srgb"|"s-rgb"|"srgb_encoded"|"srgb - display"=>InputSpace::Srgb,
        "linear_rec2020_signed_log"|"rec2020_linear_signed_log"|"linear rec.2020 signed-log"|"rec.2020 linear signed-log"=>InputSpace::Rec2020LinearSignedLog,
        other=>return Err(format!("Unknown LUT input '{other}'; refusing unsafe color mapping"))
    };
    if let Some(c)=c{
        if c.output_space.as_deref().unwrap_or("sRGB").to_ascii_lowercase()!="srgb"{return Err("MVP only supports sRGB output LUTs".into())}
        if input==InputSpace::Rec2020LinearSignedLog && c.shaper.as_deref()!=Some("noirraw_signed_log_v1") && c.shaper.as_deref()!=Some("spektrafilm_signed_log_v1") {return Err("Scene-linear LUT requires documented signed-log shaper metadata".into())}
        if let Some(stage)=&c.stage {if stage!="full_film_print" {return Err("MVP currently accepts only full film+print LUTs".into())}}
    } else if input==InputSpace::Rec2020LinearSignedLog {return Err("Scene-linear LUT needs a .json contract file".into())}
    Ok(input)
}

fn normalize(s:&str)->String {s.trim().trim_start_matches("./").replace('\\',"/").to_ascii_lowercase()}
fn lookup<'a>(row:&'a HashMap<String,String>,names:&[&str])->&'a str{
    for &n in names {if let Some(x)=row.get(n){if !x.trim().is_empty(){return x}}} ""
}
fn read_catalog(folder:&Path)->Vec<HashMap<String,String>>{
    let path=folder.join("LUT_Catalog.csv");
    let mut result=Vec::new();let Ok(mut rdr)=csv::ReaderBuilder::new().flexible(true).from_path(path) else{return result};
    let headers=rdr.headers().ok().map(|h|h.iter().map(|v|normalize(v).replace(' ',"_")).collect::<Vec<_>>()).unwrap_or_default();
    for record in rdr.records().flatten(){let mut row=HashMap::new();for (k,v) in headers.iter().zip(record.iter()){row.insert(k.clone(),v.to_string());}result.push(row);}
    result
}

pub fn index_folder(root:&Path)->Result<Vec<LutEntry>,String>{
    if !root.is_dir(){return Err(format!("LUT folder does not exist: {}",root.display()))}
    let rows=read_catalog(root);
    let mut file_paths=Vec::new();
    for e in walkdir::WalkDir::new(root).follow_links(false).into_iter().filter_map(Result::ok){
        if e.file_type().is_file()&&e.path().extension().map_or(false,|x|x.eq_ignore_ascii_case("cube")){file_paths.push(e.path().to_path_buf());}
    }
    file_paths.sort();let mut entries=Vec::new();
    for path in file_paths{
        let rel=path.strip_prefix(root).unwrap_or(&path).to_string_lossy().into_owned();let basename=path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let candidates:Vec<_>=rows.iter().map(|row|{
            let key=lookup(row,&["relative_path","lut_path","file_path","path","filename","file","lut_file","lut_filename"]);
            (row,normalize(key))
        }).collect();
        let exact:Vec<_>=candidates.iter().filter(|(_,key)|!key.is_empty()&&key==&normalize(&rel)).collect();
        let basename_hits:Vec<_>=candidates.iter().filter(|(_,key)|!key.is_empty()&&key==&normalize(&basename)).collect();
        // Prefer the full relative path, and only use basename if unique.
        let row=if exact.len()==1{Some(exact[0].0)}else if exact.is_empty()&&basename_hits.len()==1{Some(basename_hits[0].0)}else{None};
        let film=row.map(|r|lookup(r,&["film","film_stock","film_profile","negative"]).to_owned()).unwrap_or_default();
        let paper=row.map(|r|lookup(r,&["paper","print","print_paper","print_profile"]).to_owned()).unwrap_or_default();
        let stage=row.map(|r|lookup(r,&["stage","lut_stage","topology"]).to_owned()).filter(|s|!s.is_empty()).unwrap_or_else(||"Full Film + Print".into());
        let csv_input=row.map(|r|lookup(r,&["input_space","input_color_space","input_colorspace"]).to_owned()).unwrap_or_default();
        let name=row.map(|r|lookup(r,&["display_name","friendly_name","name","label","title"]).to_owned()).filter(|s|!s.is_empty());
        let fallback=path.file_stem().unwrap_or_default().to_string_lossy().replace('_'," ").replace('-'," ");
        let label=name.unwrap_or_else(||if !film.is_empty()||!paper.is_empty(){format!("{}{}{}",film,if !film.is_empty()&&!paper.is_empty(){"  ·  "}else{""},paper)}else{fallback});
        // Do not silently turn a badly declared linear LUT into sRGB.
        let contract=match contract_for(&path){Ok(x)=>x,Err(_)=>continue};
        let Ok(input)=contract_input(contract.as_ref(),&csv_input) else {continue};
        entries.push(LutEntry{path,label,film,paper,stage,input});
    }
    Ok(entries)
}

pub fn load_entry(entry:&LutEntry)->Result<Cube,String>{
    parse_cube(&fs::read_to_string(&entry.path).map_err(|e|e.to_string())?)
}

#[cfg(test)]mod tests{
    use super::*;
    fn identity(n:usize)->String{let mut out=format!("TITLE \"test\"\nLUT_3D_SIZE {n}\n");for b in 0..n{for g in 0..n{for r in 0..n{out+=&format!("{} {} {}\n",r as f32/(n-1) as f32,g as f32/(n-1) as f32,b as f32/(n-1) as f32);}}}out}
    #[test]fn cube_identity_order(){let c=parse_cube(&identity(2)).unwrap();assert_eq!(c.data.len(),8);assert_eq!(c.data[1],[1.,0.,0.,1.]);assert_eq!(c.data[4],[0.,0.,1.,1.]);}
    #[test]fn malformed_rejected(){assert!(parse_cube("LUT_3D_SIZE 3\n0 0 0").is_err());assert!(parse_cube("LUT_1D_SIZE 5").is_err());}
    #[test]fn catalog_quotes_and_names(){let root=std::env::temp_dir().join(format!("noirraw-csv-test-{}",std::process::id()));let _=fs::remove_dir_all(&root);fs::create_dir_all(root.join("nested")).unwrap();fs::write(root.join("LUT_Catalog.csv"),"relative_path,film_profile,print_profile,input_color_space\nnested/portra.cube,\"Kodak, Portra 400\",Endura,sRGB\n").unwrap();fs::write(root.join("nested/portra.cube"),identity(2)).unwrap();let items=index_folder(&root).unwrap();assert_eq!(items.len(),1);assert_eq!(items[0].label,"Kodak, Portra 400  ·  Endura");let _=fs::remove_dir_all(root);}
    #[test]fn linear_shaper_contract_required(){assert!(contract_input(None,"linear_rec2020_signed_log").is_err());}
}
