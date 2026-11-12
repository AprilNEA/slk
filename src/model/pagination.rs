use serde::Serialize;

pub(crate) const MAX_PAGE_SIZE: u16 = 999;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct CursorPage {
    pub limit: u16,
    pub cursor: Option<String>,
    pub max_pages: u16,
}

#[derive(Debug, Serialize)]
pub(crate) struct Page<T> {
    pub data: Vec<T>,
    pub pagination: Pagination,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(crate) enum Pagination {
    Cursor {
        next_cursor: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        has_more: Option<bool>,
    },
    Number {
        next_page: Option<u32>,
        has_more: bool,
    },
}

#[derive(Debug, Serialize)]
pub(crate) struct Resolution<T> {
    pub status: ResolutionStatus,
    #[serde(flatten)]
    pub candidates: Page<T>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ResolutionStatus {
    Unique,
    Ambiguous,
    NotFound,
    Incomplete,
}

impl<T> Page<T> {
    pub(crate) fn resolve(self, request: &CursorPage) -> Resolution<T> {
        let complete = request.cursor.is_none()
            && matches!(
                self.pagination,
                Pagination::Cursor {
                    next_cursor: None,
                    has_more: None | Some(false),
                }
            );
        let status = match self.data.len() {
            2.. => ResolutionStatus::Ambiguous,
            _ if !complete => ResolutionStatus::Incomplete,
            1 => ResolutionStatus::Unique,
            _ => ResolutionStatus::NotFound,
        };
        Resolution {
            status,
            candidates: self,
        }
    }
}
