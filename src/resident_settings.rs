use eframe::egui;

impl crate::app::App {
    pub(crate) fn resident_order_settings(&mut self,ui:&mut egui::Ui){self.component_order_settings(ui);}
}
