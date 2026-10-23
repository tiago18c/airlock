use crate::{ClientError, ClientRequest, ClientResponse, ClientResult};

#[derive(Clone)]
pub struct HttpProvider {
    client: reqwest::Client,
    headers: reqwest::header::HeaderMap,
    url: String,
    /// How long one request may take. `None` waits for as long as the
    /// platform does.
    timeout: Option<std::time::Duration>,
}

impl HttpProvider {
    pub fn new(url: &str) -> Self {
        Self {
            client: reqwest::Client::new(),
            headers: reqwest::header::HeaderMap::new(),
            url: url.to_owned(),
            timeout: None,
        }
    }

    pub fn new_with_timeout(url: &str, timeout: std::time::Duration) -> Self {
        Self {
            timeout: Some(timeout),
            ..Self::new(url)
        }
    }
}

impl HttpProvider {
    pub async fn send(&self, request: &ClientRequest) -> ClientResult<ClientResponse> {
        let client = &self.client;
        let url = self.url.clone();
        let headers = self.headers.clone();

        let mut builder = client.post(&url).headers(headers).json(&request);
        if let Some(timeout) = self.timeout {
            builder = builder.timeout(timeout);
        }
        let request_result: serde_json::Value = builder
            .send()
            .await
            .map_err(ClientError::from)?
            .json()
            .await
            .map_err(ClientError::from)?;

        match serde_json::from_value::<ClientResponse>(request_result.clone()) {
            Ok(response) => Ok(response),
            Err(_err) => {
                if let Ok(err) = serde_json::from_value::<ClientError>(request_result.clone()) {
                    Err(err)
                } else {
                    Err(ClientError::new(&request_result.to_string()))
                }
            }
        }
    }
}

#[derive(Clone)]
pub enum Provider {
    Http(HttpProvider),
}

impl Provider {
    pub fn new(url: &str) -> Self {
        Self::Http(HttpProvider::new(url))
    }

    pub fn new_with_timeout(url: &str, timeout: std::time::Duration) -> Self {
        Self::Http(HttpProvider::new_with_timeout(url, timeout))
    }
}
