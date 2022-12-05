use anyhow::Result;
use tui::{
	backend::Backend,
	layout::Rect,
	style::{Color, Modifier, Style},
	text::{Span, Spans},
	widgets::Tabs,
	Frame,
};

use crate::{constants::Focus, events::Key};

use super::{container::render_container, EventState, RenderAbleComponent};

#[derive(Copy, Clone, Debug)]
pub enum MenuItem {
	Home,
	File,
}

impl From<MenuItem> for usize {
	fn from(input: MenuItem) -> usize {
		match input {
			MenuItem::Home => 0,
			MenuItem::File => 1,
		}
	}
}

pub struct MenuContainerComponent {
	pub active_menu_item: MenuItem,
}

const DEFAULT_ACTIVE_TAB: MenuItem = MenuItem::Home;

impl MenuContainerComponent {
	pub fn new() -> Self {
		MenuContainerComponent {
			active_menu_item: DEFAULT_ACTIVE_TAB,
		}
	}

	pub fn active_focus(&self) -> Focus {
		match self.active_menu_item {
			MenuItem::Home => Focus::HomeTabBody,
			MenuItem::File => Focus::FileTabBody,
		}
	}

	pub fn set_active(&mut self, active_menu_item: MenuItem) {
		self.active_menu_item = active_menu_item;
	}

	pub async fn event(&mut self, key: Key) -> Result<EventState> {
		if key == Key::Char('h') {
			self.set_active(MenuItem::Home);
			return Ok(EventState::Consumed);
		}
		if key == Key::Char('f') {
			self.set_active(MenuItem::File);
			return Ok(EventState::Consumed);
		}
		Ok(EventState::NotConsumed)
	}
}

impl RenderAbleComponent for MenuContainerComponent {
	fn render<B: Backend>(
		&self,
		f: &mut Frame<B>,
		area: Rect,
		focused: bool,
	) -> Result<(), anyhow::Error> {
		let menu_titles = vec!["EDMA", "Home", "File", "Help", "Github", "Quit"];

		let menu = menu_titles
			.iter()
			.enumerate()
			.map(|(index, t)| {
				if index == 0 {
					Spans::from(vec![Span::styled(*t, Style::default().fg(Color::Yellow))])
				} else {
					let (first, rest) = t.split_at(1);
					Spans::from(vec![
						Span::styled(
							first,
							Style::default().fg(Color::Yellow).add_modifier(Modifier::UNDERLINED),
						),
						Span::styled(rest, Style::default().fg(Color::White)),
					])
				}
			})
			.collect();

		let tabs = Tabs::new(menu)
			.select(self.active_menu_item.into())
			.block(render_container("Menu", focused))
			.divider(Span::raw("|"));

		f.render_widget(tabs, area);
		Ok(())
	}
}