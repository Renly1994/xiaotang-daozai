//! 显式翻译界面文案，不转换邮件、待办或用户输入。
use std::{collections::HashMap,sync::OnceLock};

#[derive(Clone,Copy,Debug,Default,PartialEq,Eq)]
pub enum Language {System,#[default] Chinese,English}

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub enum Locale {ZhCn,En}

impl Language {
    pub const ALL:[Self;3]=[Self::System,Self::Chinese,Self::English];
    pub fn key(self)->&'static str {match self {Self::System=>"system",Self::Chinese=>"zh-CN",Self::English=>"en"}}
    pub fn parse(key:&str)->Self {match key {"system"|"auto"=>Self::System,"en"|"en-US"=>Self::English,_=>Self::Chinese}}
    pub fn resolve(self)->Locale {match self {Self::System=>system_locale(),Self::Chinese=>Locale::ZhCn,Self::English=>Locale::En}}
    pub fn label(self,locale:Locale)->&'static str {match self {Self::System=>locale.text("跟随系统"),Self::Chinese=>"简体中文",Self::English=>"English"}}
}

impl Locale {
    pub fn text<'a>(self,source:&'a str)->&'a str {
        if self==Self::ZhCn{return source;}
        static EN:OnceLock<HashMap<String,String>>=OnceLock::new();
        EN.get_or_init(||serde_json::from_str(include_str!("../assets/locales/en.json")).expect("bundled English translations"))
            .get(source).map(String::as_str).unwrap_or(source)
    }
    pub fn is_english(self)->bool {self==Self::En}
    pub fn weekday(self,monday_index:usize)->&'static str {
        (match self {Self::ZhCn=>["周一","周二","周三","周四","周五","周六","周日"],Self::En=>["Mon","Tue","Wed","Thu","Fri","Sat","Sun"]})[monday_index%7]
    }
}

pub fn locale_from_tag(tag:&str)->Locale {
    if tag.trim().split(['-','_','.']).next().is_some_and(|language|language.eq_ignore_ascii_case("zh")){Locale::ZhCn}else{Locale::En}
}

fn system_locale()->Locale {
    #[cfg(windows)] {
        // 使用显示语言，避免把地区或日期格式误认为界面语言。
        let language=unsafe{windows::Win32::Globalization::GetUserDefaultUILanguage()};
        if language & 0x03ff==0x0004 {Locale::ZhCn}else{Locale::En}
    }
    #[cfg(not(windows))] {
        let language=["LC_ALL","LC_MESSAGES","LANG"].into_iter().find_map(|key|std::env::var(key).ok().filter(|value|!value.is_empty()));
        locale_from_tag(language.as_deref().unwrap_or("en"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn language_preferences_round_trip_and_keep_legacy_chinese_default() {
        for language in Language::ALL {assert_eq!(Language::parse(language.key()),language);}
        assert_eq!(Language::parse("invalid"),Language::Chinese);
        assert_eq!(Language::default(),Language::Chinese);
        assert_eq!(Language::English.resolve(),Locale::En);
        assert_eq!(Language::Chinese.resolve(),Locale::ZhCn);
    }
    #[test]
    fn locale_detection_handles_regions_and_unsupported_languages() {
        for tag in ["zh-CN","zh_TW.UTF-8","ZH-Hans-CN"]{assert_eq!(locale_from_tag(tag),Locale::ZhCn);}
        for tag in ["en-US","fr-FR","", "ja_JP.UTF-8"]{assert_eq!(locale_from_tag(tag),Locale::En);}
    }
    #[test]
    fn explicit_translation_leaves_unrecognized_user_content_untouched() {
        assert_eq!(Locale::En.text("世界时钟"),"World clock");
        assert_eq!(Locale::ZhCn.text("世界时钟"),"世界时钟");
        assert_eq!(Locale::En.text("用户自己的会议名称"),"用户自己的会议名称");
    }
    #[test]
    fn clock_periods_follow_language_without_changing_time_format_preferences() {
        let mut settings=crate::model::Settings::default();
        settings.language=Language::English;settings.clock_12h=true;settings.clock_period=1;
        assert_eq!(settings.clock_text("00:00:00"),"12:00 AM");
        assert_eq!(settings.clock_text("12:00:00"),"12:00 PM");
        settings.clock_period=2;
        assert_eq!(settings.clock_text("09:00:00"),"9:00 Morning");
        assert_eq!(settings.clock_text("13:00:00"),"1:00 Afternoon");
        settings.clock_period=1;
        settings.language=Language::Chinese;
        assert_eq!(settings.clock_text("12:00:00"),"下午 12:00");
        assert!(settings.clock_12h);assert_eq!(settings.clock_period,1);
    }
}
