//! Label/control form layout for a settings pane, built on `NSGridView`.
//!
//! Column 0 holds trailing-aligned labels. The control column follows, then
//! `accessory_columns` columns for buttons or views that sit after a control
//! (for example "Capture…" or "Check"). A control spans every accessory column
//! its row does not use, so trailing accessories line up across rows. Rows
//! align on the first baseline, so a label sits on the text baseline of its
//! control.

use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSGridCell, NSGridCellPlacement, NSGridRow, NSGridRowAlignment, NSGridView,
    NSLayoutConstraintOrientation, NSLayoutPriorityDefaultHigh, NSLayoutPriorityDefaultLow,
    NSTextField, NSView,
};
use objc2_foundation::{NSArray, NSRange, NSString};

use super::controls::{for_auto_layout, form_label, hint_label, section_header};

const ROW_SPACING: f64 = 8.0;
const COLUMN_SPACING: f64 = 8.0;
const SECTION_TOP_PADDING: f64 = 12.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlWidth {
    /// The control fills the width of its cells.
    Fill,
    /// The control keeps its intrinsic width.
    Intrinsic,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RowAlignment {
    /// Label and control share their first text baseline.
    FirstBaseline,
    /// Every view in the row is vertically centered, for rows whose control
    /// has no text baseline (a slider or a meter).
    Center,
}

pub struct FormGrid {
    mtm: MainThreadMarker,
    grid: Retained<NSGridView>,
    column_count: usize,
}

impl FormGrid {
    pub fn new(mtm: MainThreadMarker, accessory_columns: usize) -> Self {
        let column_count = 2 + accessory_columns;
        let grid = for_auto_layout(NSGridView::gridViewWithNumberOfColumns_rows(
            column_count as isize,
            0,
            mtm,
        ));
        grid.setRowAlignment(NSGridRowAlignment::FirstBaseline);
        grid.setRowSpacing(ROW_SPACING);
        grid.setColumnSpacing(COLUMN_SPACING);
        grid.columnAtIndex(0)
            .setXPlacement(NSGridCellPlacement::Trailing);
        for column in 1..column_count {
            grid.columnAtIndex(column as isize)
                .setXPlacement(NSGridCellPlacement::Leading);
        }

        Self {
            mtm,
            grid,
            column_count,
        }
    }

    pub fn view(&self) -> &NSGridView {
        &self.grid
    }

    /// Adds a bold header spanning every column. Headers after the first row
    /// get extra space above them to separate sections.
    pub fn add_section_header(&self, title: &str) {
        let header = section_header(self.mtm, title);
        let is_first_row = self.grid.numberOfRows() == 0;
        let row = self.add_row_views(&[&header]);
        merge_cells(&row, 0, self.column_count);
        // The merged cell would otherwise inherit the label column's trailing
        // placement.
        if let Some(cell) = self.grid.cellForView(&header) {
            cell.setXPlacement(NSGridCellPlacement::Leading);
        }
        if !is_first_row {
            row.setTopPadding(SECTION_TOP_PADDING);
        }
    }

    /// Adds `label` followed by `control` and its trailing `accessories`.
    pub fn add_row(
        &self,
        label: &str,
        control: &NSView,
        width: ControlWidth,
        accessories: &[&NSView],
    ) {
        self.add_aligned_row(
            label,
            control,
            width,
            accessories,
            RowAlignment::FirstBaseline,
        );
    }

    pub fn add_aligned_row(
        &self,
        label: &str,
        control: &NSView,
        width: ControlWidth,
        accessories: &[&NSView],
        alignment: RowAlignment,
    ) {
        assert!(
            accessories.len() < self.column_count - 1,
            "a form grid row has at most {} accessories",
            self.column_count - 2
        );
        assert!(
            accessories.is_empty() || width == ControlWidth::Fill,
            "a control followed by accessories fills its cells"
        );
        let label = form_label(self.mtm, label);
        for accessory in accessories {
            accessory.setContentHuggingPriority_forOrientation(
                NSLayoutPriorityDefaultHigh,
                NSLayoutConstraintOrientation::Horizontal,
            );
        }

        let control_span = self.column_count - 1 - accessories.len();
        let empty = NSGridCell::emptyContentView(self.mtm);
        let mut views: Vec<&NSView> = vec![&label, control];
        views.extend(std::iter::repeat_n(&*empty, control_span - 1));
        views.extend_from_slice(accessories);
        let row = self.add_row_views(&views);
        merge_cells(&row, 1, control_span);
        // Accessories in the last column fill it, so buttons stacked in that
        // column share one width and the grid's trailing edge.
        if let Some(cell) = accessories
            .last()
            .and_then(|accessory| self.grid.cellForView(accessory))
        {
            cell.setXPlacement(NSGridCellPlacement::Fill);
        }
        self.hug_label_and_accessory_columns(&label, control, accessories);
        self.set_control_width(control, width);

        if alignment == RowAlignment::Center {
            row.setRowAlignment(NSGridRowAlignment::None);
            row.setYPlacement(NSGridCellPlacement::Center);
        }
    }

    /// Adds a view without a label that spans the control and accessory
    /// columns: a checkbox, or a hint below the row above it.
    pub fn add_detail_row(&self, view: &NSView, width: ControlWidth) -> Retained<NSGridRow> {
        let empty = NSGridCell::emptyContentView(self.mtm);
        let row = self.add_row_views(&[&empty, view]);
        merge_cells(&row, 1, self.column_count - 1);
        self.set_control_width(view, width);
        row
    }

    /// Adds a hint below the row above it. The row is hidden while the hint
    /// has no text, so an unused hint leaves no gap.
    pub fn add_hint_row(&self, text: Option<&str>) -> HintRow {
        let label = hint_label(self.mtm, "");
        let row = self.add_detail_row(&label, ControlWidth::Fill);
        let hint = HintRow { label, row };
        hint.set_text(text);
        hint
    }

    /// Adds a row with `views` in its leading columns and empty cells after
    /// them.
    fn add_row_views(&self, views: &[&NSView]) -> Retained<NSGridRow> {
        let empty = NSGridCell::emptyContentView(self.mtm);
        let mut row_views = views.to_vec();
        row_views.resize(self.column_count, &empty);
        self.grid.addRowWithViews(&NSArray::from_slice(&row_views))
    }

    /// Keeps the label and accessory columns at the width of their widest
    /// content, so the control column takes all remaining width.
    ///
    /// The label is pulled towards the grid's leading edge, and each
    /// accessory's leading edge towards the view before it (the control fills
    /// its cells, so its trailing edge is the control column's). Only the
    /// widest view of a column can satisfy its pull, which makes that column
    /// exactly as wide as it.
    fn hug_label_and_accessory_columns(
        &self,
        label: &NSView,
        control: &NSView,
        accessories: &[&NSView],
    ) {
        let mut pulls = vec![label
            .leadingAnchor()
            .constraintEqualToAnchor(&self.grid.leadingAnchor())];
        let mut previous = control;
        for accessory in accessories {
            pulls.push(
                accessory
                    .leadingAnchor()
                    .constraintEqualToAnchor_constant(&previous.trailingAnchor(), COLUMN_SPACING),
            );
            previous = accessory;
        }
        for pull in &pulls {
            pull.setPriority(NSLayoutPriorityDefaultLow);
            pull.setActive(true);
        }
    }

    fn set_control_width(&self, control: &NSView, width: ControlWidth) {
        if width == ControlWidth::Fill {
            if let Some(cell) = self.grid.cellForView(control) {
                cell.setXPlacement(NSGridCellPlacement::Fill);
            }
        }
    }
}

#[derive(Debug)]
pub struct HintRow {
    label: Retained<NSTextField>,
    row: Retained<NSGridRow>,
}

impl HintRow {
    pub fn set_text(&self, text: Option<&str>) {
        let text = text.unwrap_or_default();
        self.label.setStringValue(&NSString::from_str(text));
        self.row.setHidden(text.is_empty());
    }
}

fn merge_cells(row: &NSGridRow, first_column: usize, column_count: usize) {
    if column_count > 1 {
        row.mergeCellsInRange(NSRange::new(first_column, column_count));
    }
}
