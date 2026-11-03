use extism_pdk::{extism::error_set, input, output, Error, Json, Memory};

pub(crate) fn return_error(e: Error) -> i32 {
    let err = format!("{e:?}");
    let mem = Memory::from_bytes(&err).unwrap();
    unsafe {
        error_set(mem.offset());
    }
    -1
}

macro_rules! try_input_json {
    () => {{
        let x = input();
        match x {
            Ok(Json(x)) => x,
            Err(e) => return return_error(e),
        }
    }};
}

#[no_mangle]
pub extern "C" fn call_tool() -> i32 {
    let ret = crate::call_tool(try_input_json!()).and_then(|x| output(Json(x)));

    match ret {
        Ok(()) => 0,
        Err(e) => return_error(e),
    }
}

#[no_mangle]
pub extern "C" fn complete() -> i32 {
    let ret = crate::complete(try_input_json!()).and_then(|x| output(Json(x)));

    match ret {
        Ok(()) => 0,
        Err(e) => return_error(e),
    }
}

#[no_mangle]
pub extern "C" fn get_prompt() -> i32 {
    let ret = crate::get_prompt(try_input_json!()).and_then(|x| output(Json(x)));

    match ret {
        Ok(()) => 0,
        Err(e) => return_error(e),
    }
}

#[no_mangle]
pub extern "C" fn list_prompts() -> i32 {
    let ret = crate::list_prompts(try_input_json!()).and_then(|x| output(Json(x)));

    match ret {
        Ok(()) => 0,
        Err(e) => return_error(e),
    }
}

#[no_mangle]
pub extern "C" fn list_resource_templates() -> i32 {
    let ret = crate::list_resource_templates(try_input_json!()).and_then(|x| output(Json(x)));

    match ret {
        Ok(()) => 0,
        Err(e) => return_error(e),
    }
}

#[no_mangle]
pub extern "C" fn list_resources() -> i32 {
    let ret = crate::list_resources(try_input_json!()).and_then(|x| output(Json(x)));

    match ret {
        Ok(()) => 0,
        Err(e) => return_error(e),
    }
}

#[no_mangle]
pub extern "C" fn list_tools() -> i32 {
    let ret = crate::list_tools(try_input_json!()).and_then(|x| output(Json(x)));

    match ret {
        Ok(()) => 0,
        Err(e) => return_error(e),
    }
}

#[no_mangle]
pub extern "C" fn on_roots_list_changed() -> i32 {
    let ret = crate::on_roots_list_changed(try_input_json!()).and_then(output);

    match ret {
        Ok(()) => 0,
        Err(e) => return_error(e),
    }
}

#[no_mangle]
pub extern "C" fn read_resource() -> i32 {
    let ret = crate::read_resource(try_input_json!()).and_then(|x| output(Json(x)));

    match ret {
        Ok(()) => 0,
        Err(e) => return_error(e),
    }
}
