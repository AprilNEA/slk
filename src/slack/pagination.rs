use crate::error::{Error, Result};
use futures_util::{StreamExt, TryStreamExt, stream};
use serde::Serialize;
use slack_morphism::SlackResponseMetadata;
use std::collections::HashSet;
use std::future::Future;
use std::hash::Hash;

pub(crate) const MAX_PAGE_SIZE: u16 = 999;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct CursorPage {
    pub limit: u16,
    pub cursor: Option<String>,
    pub max_pages: u16,
}

pub(super) trait RequestPage: Clone + Eq + Hash {
    fn max_pages(&self) -> u16;
    fn advance(&mut self, pagination: &Pagination) -> Result<bool>;
}

impl RequestPage for CursorPage {
    fn max_pages(&self) -> u16 {
        self.max_pages
    }

    fn advance(&mut self, pagination: &Pagination) -> Result<bool> {
        let Pagination::Cursor { next_cursor, .. } = pagination else {
            return Err(Error::Response);
        };
        self.cursor.clone_from(next_cursor);
        Ok(self.cursor.is_some())
    }
}

pub(super) async fn collect<P, T, F, Fut>(request: &P, fetch: F) -> Result<Page<T>>
where
    P: RequestPage + Send + Sync,
    T: Send,
    F: Fn(P) -> Fut + Send + Sync,
    Fut: Future<Output = Result<Page<T>>> + Send,
{
    let fetch = &fetch;
    let pages = stream::try_unfold(
        (Some(request.clone()), HashSet::new()),
        |(request, mut seen)| async move {
            let Some(mut request) = request else {
                return Ok(None);
            };
            if !seen.insert(request.clone()) {
                return Err(Error::Response);
            }
            let page = fetch(request.clone()).await?;
            let next = request.advance(&page.pagination)?.then_some(request);
            Ok(Some((page, (next, seen))))
        },
    );
    pages
        .take(usize::from(request.max_pages()))
        .try_fold(None, |previous: Option<Page<T>>, mut page| async move {
            if let Some(mut previous) = previous {
                previous.data.append(&mut page.data);
                page.data = previous.data;
            }
            Ok(Some(page))
        })
        .await?
        .ok_or(Error::Response)
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
    pub(super) fn cursor(
        data: Vec<T>,
        metadata: Option<SlackResponseMetadata>,
        has_more: Option<bool>,
    ) -> Self {
        Self {
            data,
            pagination: Pagination::Cursor {
                next_cursor: metadata
                    .and_then(|value| value.next_cursor.map(|cursor| cursor.0))
                    .filter(|cursor| !cursor.trim().is_empty()),
                has_more,
            },
        }
    }

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
