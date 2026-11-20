use super::*;

pub(super) async fn handle_viewport(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let width = cmd.get("width").and_then(|v| v.as_i64()).unwrap_or(1280) as i32;
    let height = cmd.get("height").and_then(|v| v.as_i64()).unwrap_or(720) as i32;
    let scale = cmd
        .get("deviceScaleFactor")
        .and_then(|v| v.as_f64())
        .unwrap_or(1.0);
    let mobile = cmd.get("mobile").and_then(|v| v.as_bool()).unwrap_or(false);

    mgr.set_viewport(width, height, scale, mobile).await?;

    state.viewport = Some((width, height, scale, mobile));

    // Update stream server viewport so status messages and screencast use the new dimensions
    if let Some(ref server) = state.stream_server {
        server.set_viewport(width as u32, height as u32).await;
    }

    Ok(json!({ "width": width, "height": height, "deviceScaleFactor": scale, "mobile": mobile }))
}

pub(super) async fn handle_user_agent(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let ua = cmd
        .get("userAgent")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'userAgent' parameter")?;
    mgr.set_user_agent(ua).await?;
    state.session_setup.user_agent = Some(ua.to_string());
    Ok(json!({ "userAgent": ua }))
}

pub(super) async fn handle_set_media(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let media = cmd.get("media").and_then(|v| v.as_str());

    let mut feat_list: Vec<(String, String)> = Vec::new();

    if let Some(scheme) = cmd.get("colorScheme").and_then(|v| v.as_str()) {
        feat_list.push(("prefers-color-scheme".to_string(), scheme.to_string()));
    }
    if let Some(motion) = cmd.get("reducedMotion").and_then(|v| v.as_str()) {
        feat_list.push(("prefers-reduced-motion".to_string(), motion.to_string()));
    }

    if let Some(obj) = cmd.get("features").and_then(|v| v.as_object()) {
        for (k, v) in obj {
            feat_list.push((k.clone(), v.as_str().unwrap_or("").to_string()));
        }
    }

    let features = if feat_list.is_empty() {
        None
    } else {
        Some(feat_list.clone())
    };

    mgr.set_emulated_media(media, features).await?;
    state.session_setup.emulated_media = Some(EmulatedMedia {
        media: media.map(String::from),
        features: feat_list,
    });
    Ok(json!({ "set": true }))
}

pub(super) async fn handle_bringtofront(state: &DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    mgr.bring_to_front().await?;
    Ok(json!({ "broughtToFront": true }))
}

pub(super) async fn handle_timezone(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let timezone = cmd
        .get("timezoneId")
        .or_else(|| cmd.get("timezone"))
        .and_then(|v| v.as_str())
        .ok_or("Missing 'timezoneId' parameter")?;
    mgr.set_timezone(timezone).await?;
    state.session_setup.timezone = Some(timezone.to_string());
    Ok(json!({ "timezoneId": timezone }))
}

pub(super) async fn handle_locale(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let locale = cmd
        .get("locale")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'locale' parameter")?;
    mgr.set_locale(locale).await?;
    state.session_setup.locale = Some(locale.to_string());
    Ok(json!({ "locale": locale }))
}

pub(super) async fn handle_geolocation(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let latitude = cmd
        .get("latitude")
        .and_then(|v| v.as_f64())
        .ok_or("Missing 'latitude' parameter")?;
    let longitude = cmd
        .get("longitude")
        .and_then(|v| v.as_f64())
        .ok_or("Missing 'longitude' parameter")?;
    let accuracy = cmd.get("accuracy").and_then(|v| v.as_f64());

    mgr.set_geolocation(latitude, longitude, accuracy).await?;
    state.session_setup.geolocation = Some((latitude, longitude, accuracy));
    Ok(json!({ "latitude": latitude, "longitude": longitude }))
}

pub(super) async fn handle_permissions(cmd: &Value, state: &DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let permissions: Vec<String> = cmd
        .get("permissions")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    mgr.grant_permissions(&permissions).await?;
    Ok(json!({ "granted": permissions }))
}

pub(super) async fn handle_device(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let name = cmd
        .get("name")
        .or_else(|| cmd.get("device"))
        .and_then(|v| v.as_str())
        .ok_or("Missing 'name' parameter")?;

    let (width, height, scale, mobile, ua) = match name.to_lowercase().as_str() {
        "iphone 15" | "iphone15" => (393, 852, 3.0, true, "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1"),
        "iphone 16" | "iphone16" => (393, 852, 3.0, true, "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1"),
        "iphone 16 pro" | "iphone16pro" => (402, 874, 3.0, true, "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1"),
        "iphone 17" | "iphone17" => (402, 874, 3.0, true, "Mozilla/5.0 (iPhone; CPU iPhone OS 19_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/19.0 Mobile/15E148 Safari/604.1"),
        "ipad" | "ipad air" => (820, 1180, 2.0, true, "Mozilla/5.0 (iPad; CPU OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/604.1"),
        "ipad pro" => (1024, 1366, 2.0, true, "Mozilla/5.0 (iPad; CPU OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/604.1"),
        "pixel 9" | "pixel9" => (412, 923, 2.625, true, "Mozilla/5.0 (Linux; Android 15; Pixel 9) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Mobile Safari/537.36"),
        "galaxy s25" | "galaxys25" => (360, 800, 3.0, true, "Mozilla/5.0 (Linux; Android 15; SM-S931B) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Mobile Safari/537.36"),
        // Legacy aliases
        "iphone 12" | "iphone12" => (390, 844, 3.0, true, "Mozilla/5.0 (iPhone; CPU iPhone OS 14_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/14.0 Mobile/15E148 Safari/604.1"),
        "iphone 14" | "iphone14" => (390, 844, 3.0, true, "Mozilla/5.0 (iPhone; CPU iPhone OS 16_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.0 Mobile/15E148 Safari/604.1"),
        "pixel 5" | "pixel5" => (393, 851, 2.75, true, "Mozilla/5.0 (Linux; Android 11; Pixel 5) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/90.0.4430.91 Mobile Safari/537.36"),
        "pixel 7" | "pixel7" => (412, 915, 2.625, true, "Mozilla/5.0 (Linux; Android 13; Pixel 7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/116.0.0.0 Mobile Safari/537.36"),
        "galaxy s21" | "galaxys21" => (360, 800, 3.0, true, "Mozilla/5.0 (Linux; Android 11; SM-G991B) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/90.0.4430.91 Mobile Safari/537.36"),
        _ => return Err(format!("Unknown device: {}. Supported: iPhone 15, iPhone 16, iPhone 16 Pro, iPhone 17, iPad, iPad Pro, Pixel 9, Galaxy S25", name)),
    };

    mgr.set_viewport(width, height, scale, mobile).await?;
    mgr.set_user_agent(ua).await?;

    state.viewport = Some((width, height, scale, mobile));
    state.session_setup.user_agent = Some(ua.to_string());

    // Update stream server viewport so status messages and screencast use the new dimensions
    if let Some(ref server) = state.stream_server {
        server.set_viewport(width as u32, height as u32).await;
    }

    Ok(json!({
        "device": name,
        "width": width,
        "height": height,
        "deviceScaleFactor": scale,
        "mobile": mobile,
    }))
}

// ---------------------------------------------------------------------------
// Stream handlers
// ---------------------------------------------------------------------------

pub(super) async fn handle_device_list() -> Result<Value, String> {
    #[cfg(target_os = "macos")]
    {
        use crate::native::webdriver::ios;
        let devices = ios::list_all_devices()?;
        Ok(ios::to_device_json(&devices))
    }

    #[cfg(not(target_os = "macos"))]
    {
        Err("device_list is only available on macOS with Xcode".to_string())
    }
}

// ---------------------------------------------------------------------------
// Input event handlers
// ---------------------------------------------------------------------------
