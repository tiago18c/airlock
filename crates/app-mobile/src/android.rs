//! Calls into Android over JNI, on the activity ndk-context holds.

use std::path::PathBuf;

use jni::objects::{JObject, JValue};
use jni::{JNIEnv, JavaVM};

/// Run `f` with a JNI environment attached to this thread and the activity.
/// A Java exception is cleared and returned as the error, so it does not
/// stay pending into the next call.
pub(crate) fn with_activity<R>(
    f: impl FnOnce(&mut JNIEnv, &JObject) -> jni::errors::Result<R>,
) -> Result<R, String> {
    let ctx = ndk_context::android_context();
    // SAFETY: ndk-context holds the VM and a global reference to the
    // activity for the life of the process; tao sets them before `main`.
    let vm = unsafe { JavaVM::from_raw(ctx.vm().cast()) }.map_err(|err| err.to_string())?;
    let mut env = vm.attach_current_thread().map_err(|err| err.to_string())?;
    // SAFETY: as above. A `JObject` made from a raw pointer does not delete
    // the reference when dropped, so the global reference stays ndk-context's.
    let activity = unsafe { JObject::from_raw(ctx.context().cast()) };
    let result = f(&mut env, &activity);
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
    result.map_err(|err| err.to_string())
}

pub(crate) fn files_dir() -> Result<PathBuf, String> {
    with_activity(|env, activity| {
        // Not getFilesDir: backups and device-to-device transfers skip this
        // folder, so the vault never leaves the phone that way.
        let dir = env
            .call_method(activity, "getNoBackupFilesDir", "()Ljava/io/File;", &[])?
            .l()?;
        let path = env
            .call_method(&dir, "getAbsolutePath", "()Ljava/lang/String;", &[])?
            .l()?;
        let path: String = env.get_string(&path.into())?.into();
        Ok(PathBuf::from(path))
    })
}

pub(crate) fn open_url(url: &str) -> Result<(), String> {
    with_activity(|env, activity| {
        let text = env.new_string(url)?;
        let uri = env
            .call_static_method(
                "android/net/Uri",
                "parse",
                "(Ljava/lang/String;)Landroid/net/Uri;",
                &[JValue::Object(&text)],
            )?
            .l()?;
        let action = env.new_string("android.intent.action.VIEW")?;
        let intent = env.new_object(
            "android/content/Intent",
            "(Ljava/lang/String;Landroid/net/Uri;)V",
            &[JValue::Object(&action), JValue::Object(&uri)],
        )?;
        env.call_method(
            activity,
            "startActivity",
            "(Landroid/content/Intent;)V",
            &[JValue::Object(&intent)],
        )?;
        Ok(())
    })
}

/// Ask Android to open the app's launcher activity in a new task, which
/// starts a new process once this one has ended.
pub(crate) fn relaunch() -> Result<(), String> {
    with_activity(|env, activity| {
        let packages = env
            .call_method(
                activity,
                "getPackageManager",
                "()Landroid/content/pm/PackageManager;",
                &[],
            )?
            .l()?;
        let name = env
            .call_method(activity, "getPackageName", "()Ljava/lang/String;", &[])?
            .l()?;
        let intent = env
            .call_method(
                &packages,
                "getLaunchIntentForPackage",
                "(Ljava/lang/String;)Landroid/content/Intent;",
                &[JValue::Object(&name)],
            )?
            .l()?;
        // FLAG_ACTIVITY_NEW_TASK | FLAG_ACTIVITY_CLEAR_TASK
        let flags = 0x1000_0000 | 0x0000_8000;
        env.call_method(
            &intent,
            "addFlags",
            "(I)Landroid/content/Intent;",
            &[JValue::Int(flags)],
        )?;
        env.call_method(
            activity,
            "startActivity",
            "(Landroid/content/Intent;)V",
            &[JValue::Object(&intent)],
        )?;
        Ok(())
    })
}

/// `SeedVaultBridge`, the Kotlin object in the app's MainActivity.kt. App
/// classes are found through the activity's class loader: a thread Rust
/// attached sees only the system's.
const BRIDGE: &str = "dev.dioxus.main.SeedVaultBridge";

fn bridge_class<'a>(
    env: &mut JNIEnv<'a>,
    activity: &JObject,
) -> jni::errors::Result<jni::objects::JClass<'a>> {
    let loader = env
        .call_method(activity, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])?
        .l()?;
    let name = env.new_string(BRIDGE)?;
    let class = env
        .call_method(
            &loader,
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&name)],
        )?
        .l()?;
    Ok(class.into())
}

pub(crate) fn bridge_availability() -> Result<i32, String> {
    with_activity(|env, activity| {
        let class = bridge_class(env, activity)?;
        env.call_static_method(&class, "availability", "()I", &[])?
            .i()
    })
}

pub(crate) fn bridge_take(id: i64) -> Result<Option<String>, String> {
    with_activity(|env, activity| {
        let class = bridge_class(env, activity)?;
        let result = env
            .call_static_method(&class, "take", "(J)Ljava/lang/String;", &[JValue::Long(id)])?
            .l()?;
        if result.is_null() {
            return Ok(None);
        }
        let text: String = env.get_string(&result.into())?.into();
        Ok(Some(text))
    })
}

pub(crate) fn bridge_authorize(id: i64, how: i32) -> Result<(), String> {
    with_activity(|env, activity| {
        let class = bridge_class(env, activity)?;
        env.call_static_method(
            &class,
            "authorize",
            "(JI)V",
            &[JValue::Long(id), JValue::Int(how)],
        )?;
        Ok(())
    })
}

pub(crate) fn bridge_public_keys(id: i64, token: u64, paths: &str) -> Result<(), String> {
    with_activity(|env, activity| {
        let class = bridge_class(env, activity)?;
        let paths = env.new_string(paths)?;
        env.call_static_method(
            &class,
            "publicKeys",
            "(JJLjava/lang/String;)V",
            &[
                JValue::Long(id),
                JValue::Long(token as i64),
                JValue::Object(&paths),
            ],
        )?;
        Ok(())
    })
}

pub(crate) fn bridge_sign(
    id: i64,
    token: u64,
    path: &str,
    payload: &str,
    message: bool,
) -> Result<(), String> {
    with_activity(|env, activity| {
        let class = bridge_class(env, activity)?;
        let path = env.new_string(path)?;
        let payload = env.new_string(payload)?;
        env.call_static_method(
            &class,
            "sign",
            "(JJLjava/lang/String;Ljava/lang/String;Z)V",
            &[
                JValue::Long(id),
                JValue::Long(token as i64),
                JValue::Object(&path),
                JValue::Object(&payload),
                JValue::Bool(message.into()),
            ],
        )?;
        Ok(())
    })
}
