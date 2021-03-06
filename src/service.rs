//! Service that interfaces with the browser to handle routing.

use yew::callback::Callback;

use crate::route::{Route, RouteState};
use cfg_if::cfg_if;
use cfg_match::cfg_match;
use std::marker::PhantomData;

cfg_if! {
    if #[cfg(feature = "std_web")] {
        use stdweb::{
            js,
            unstable::{TryFrom, TryInto},
            web::{event::PopStateEvent, window, EventListenerHandle, History, IEventTarget, Location},
            Value,
        };
    } else if #[cfg(feature = "web_sys")] {
        use web_sys::{History, Location, PopStateEvent};
        use gloo::events::EventListener;
        use wasm_bindgen::{JsValue as Value, JsCast};
    }
}

/// A service that facilitates manipulation of the browser's URL bar and responding to browser events
/// when users press 'forward' or 'back'.
///
/// The `T` determines what route state can be stored in the route service.
#[derive(Debug)]
pub struct RouteService<STATE = ()> {
    history: History,
    location: Location,
    #[cfg(feature = "std_web")]
    event_listener: Option<EventListenerHandle>,
    #[cfg(feature = "web_sys")]
    event_listener: Option<EventListener>,
    phantom_data: PhantomData<STATE>,
}

impl<STATE> Default for RouteService<STATE>
where
    STATE: RouteState,
{
    fn default() -> Self {
        RouteService::<STATE>::new()
    }
}

impl<T> RouteService<T> {
    /// Creates the route service.
    pub fn new() -> RouteService<T> {
        let (history, location) = cfg_match! {
            feature = "std_web" => ({
                (
                    window().history(),
                    window().location().expect("browser does not support location API")
                )
            }),
            feature = "web_sys" => ({
                let window = web_sys::window().unwrap();
                (
                    window.history().expect("browser does not support history API"),
                    window.location()
                )
            }),
        };

        RouteService {
            history,
            location,
            event_listener: None,
            phantom_data: PhantomData,
        }
    }

    #[inline]
    fn get_route_from_location(location: &Location) -> String {
        let path = location.pathname().unwrap();
        let query = location.search().unwrap();
        let fragment = location.hash().unwrap();
        format_route_string(&path, &query, &fragment)
    }

    /// Gets the path name of the current url.
    pub fn get_path(&self) -> String {
        self.location.pathname().unwrap()
    }

    /// Gets the query string of the current url.
    pub fn get_query(&self) -> String {
        self.location.search().unwrap()
    }

    /// Gets the fragment of the current url.
    pub fn get_fragment(&self) -> String {
        self.location.hash().unwrap()
    }
}

impl<STATE> RouteService<STATE>
where
    STATE: RouteState,
{
    /// Registers a callback to the route service.
    /// Callbacks will be called when the History API experiences a change such as
    /// popping a state off of its stack when the forward or back buttons are pressed.
    pub fn register_callback(&mut self, callback: Callback<Route<STATE>>) {
        let cb = move |event: PopStateEvent| {
            let state_value: Value = event.state();
            let state_string: String = cfg_match! {
                feature = "std_web" => String::try_from(state_value).unwrap_or_default(),
                feature = "web_sys" => state_value.as_string().unwrap_or_default(),
            };
            let state: STATE = serde_json::from_str(&state_string).unwrap_or_else(|_| {
                log::error!("Could not deserialize state string");
                STATE::default()
            });

            // Can't use the existing location, because this is a callback, and can't move it in
            // here.
            let location: Location = cfg_match! {
                feature = "std_web" => window().location().unwrap(),
                feature = "web_sys" => web_sys::window().unwrap().location(),
            };
            let route: String = Self::get_route_from_location(&location);

            callback.emit(Route { route, state })
        };

        cfg_if! {
            if #[cfg(feature = "std_web")] {
                self.event_listener = Some(window().add_event_listener(move |event: PopStateEvent| {
                    cb(event)
                }));
            } else if #[cfg(feature = "web_sys")] {
                self.event_listener = Some(EventListener::new(web_sys::window().unwrap().as_ref(), "popstate", move |event| {
                    let event: PopStateEvent = event.clone().dyn_into().unwrap();
                    cb(event)
                }));
            }
        };
    }

    /// Sets the browser's url bar to contain the provided route,
    /// and creates a history entry that can be navigated via the forward and back buttons.
    ///
    /// The route should be a relative path that starts with a `/`.
    pub fn set_route(&mut self, route: &str, state: STATE) {
        let state_string: String = serde_json::to_string(&state).unwrap_or_else(|_| {
            log::error!("Could not serialize state string");
            "".to_string()
        });
        cfg_match! {
            feature = "std_web" => ({
                self.history.push_state(state_string, "", Some(route));
            }),
            feature = "web_sys" => ({
                let _ = self.history.push_state_with_url(&Value::from_str(&state_string), "", Some(route));
            }),
        };
    }

    /// Replaces the route with another one removing the most recent history event and
    /// creating another history event in its place.
    pub fn replace_route(&mut self, route: &str, state: STATE) {
        let state_string: String = serde_json::to_string(&state).unwrap_or_else(|_| {
            log::error!("Could not serialize state string");
            "".to_string()
        });
        cfg_match! {
            feature = "std_web" => ({
                let _ = self.history.replace_state(state_string, "", Some(route));
            }),
            feature = "web_sys" => ({
                let _ = self.history.replace_state_with_url(&Value::from_str(&state_string), "", Some(route));
            }),
        };
    }

    /// Gets the concatenated path, query, and fragment.
    pub fn get_route(&self) -> Route<STATE> {
        let route_string = Self::get_route_from_location(&self.location);
        let state: STATE = get_state_string(&self.history)
            .or_else(|| {
                log::trace!("History state is empty");
                None
            })
            .and_then(|state_string| -> Option<Option<STATE>>{
                serde_json::from_str(&state_string)
                    .ok()
                    .or_else(|| {
                        log::error!("Could not deserialize state string");
                        None
                    })
            })
            .and_then(std::convert::identity) // flatten
            .unwrap_or_default();
        Route {
            route: route_string,
            state,
        }
    }
}

/// Formats a path, query, and fragment into a string.
///
/// # Note
/// This expects that all three already have their expected separators (?, #, etc)
pub(crate) fn format_route_string(path: &str, query: &str, fragment: &str) -> String {
    format!(
        "{path}{query}{fragment}",
        path = path,
        query = query,
        fragment = fragment
    )
}

fn get_state(history: &History) -> Value {
    cfg_match! {
        feature = "std_web" => js!(
            return @{history}.state;
        ),
        feature = "web_sys" => history.state().unwrap(),
    }
}

fn get_state_string(history: &History) -> Option<String> {
    cfg_match! {
        feature = "std_web" => get_state(history).try_into().ok(),
        feature = "web_sys" => get_state(history).as_string(),
    }
}
